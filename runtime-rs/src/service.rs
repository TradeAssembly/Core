// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{AuthorityContext, IdempotencyKey, ServiceRuntime, SideEffectContext};
use crate::runtime_config::{ResolvedRuntimeManifest, RuntimeBuilder, RuntimeConfig};
use crate::{ai, auth, control_plane, demos, mcp, report_envelope, spec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

mod agent_deployment;
mod attribution_journal;
mod backtest;
pub mod backtest_lifecycle;
mod broker_onboarding;
mod broker_recovery;
mod capability_graph;
mod comparisons;
pub mod connected_runner;
mod contracts;
pub mod core_runner;
mod credentials;
mod dataset_ingestion;
mod derivatives;
mod execution;
mod external_broker_evidence;
mod fill_quality;
mod journal_view;
mod lifecycle_calendar;
mod live_authorization;
mod marketdata;
mod object_authorization;
mod onboarding;
mod options;
mod orders;
pub mod paper_runner;
mod plugin_lifecycle;
mod plugin_oauth;
mod positions;
mod providers;
pub mod reach;
mod replay;
mod research;
mod research_notebook;
mod risk;
mod robustness;
pub mod robustness_lifecycle;
mod scheduler;
mod sightline;
mod strategy;
mod workers;
mod workspace;

pub const LEGAL_BOUNDARY: &str =
    "TradeAssembly never tells users what to trade, when to trade, or how much to trade.";

#[derive(Clone, Debug, PartialEq)]
pub struct ServiceResponse {
    pub status: u16,
    pub body: Value,
}

impl ServiceResponse {
    pub fn ok(body: Value) -> Self {
        Self { status: 200, body }
    }

    pub fn created(body: Value) -> Self {
        Self { status: 201, body }
    }

    pub fn bad_request(code: &str) -> Self {
        Self::error(400, code, false)
    }

    pub fn bad_request_with_details(code: &str, details: Value) -> Self {
        Self::error_with_details(400, code, false, details)
    }

    pub fn unauthorized(code: &str) -> Self {
        Self::error(401, code, false)
    }

    pub fn forbidden(code: &str) -> Self {
        Self::error(403, code, false)
    }

    pub fn forbidden_with_details(code: &str, details: Value) -> Self {
        Self::error_with_details(403, code, false, details)
    }

    pub fn conflict(code: &str) -> Self {
        Self::error(409, code, false)
    }

    pub fn conflict_with_details(code: &str, details: Value) -> Self {
        Self::error_with_details(409, code, false, details)
    }

    pub fn unprocessable_with_details(code: &str, details: Value) -> Self {
        Self::error_with_details(422, code, false, details)
    }

    pub fn bad_gateway(code: &str) -> Self {
        Self::error(502, code, true)
    }

    pub fn internal_error(_message: &str) -> Self {
        Self::error(500, "internal_error", false)
    }

    pub fn not_found(path: &str) -> Self {
        Self {
            status: 404,
            body: json!({
                "detail": "route_not_found",
                "error": {
                    "code": "route_not_found",
                    "message": format!("unknown route: {path}"),
                    "retryable": false,
                    "details": {},
                },
            }),
        }
    }

    pub fn object_unavailable() -> Self {
        Self::error(404, "object_not_available", false)
    }

    fn error(status: u16, code: &str, retryable: bool) -> Self {
        Self::error_with_details(status, code, retryable, json!({}))
    }

    fn error_with_details(status: u16, code: &str, retryable: bool, details: Value) -> Self {
        Self {
            status,
            body: json!({
                "detail": code,
                "error": {
                    "code": code,
                    "message": code,
                    "retryable": retryable,
                    "details": details,
                },
            }),
        }
    }

    fn duplicate(status: u16, command_id: &str) -> Self {
        if status >= 400 {
            return Self::error(status, "idempotency_replay_failed", false);
        }
        Self {
            status,
            body: duplicate_replay_body(
                json!({
                    "ok": true,
                    "commandId": command_id,
                }),
                command_id,
            ),
        }
    }

    fn duplicate_with_body(status: u16, command_id: &str, body: Value) -> Self {
        if status >= 400 {
            return Self::error(status, "idempotency_replay_failed", false);
        }
        Self {
            status,
            body: duplicate_replay_body(body, command_id),
        }
    }
}

fn finance_authority_denied_response(error: &str) -> ServiceResponse {
    let mut response = ServiceResponse::forbidden("finance_authority_denied");
    let parts = error
        .strip_prefix("finance_authority:")
        .unwrap_or_default()
        .split(':')
        .collect::<Vec<_>>();
    let reason = parts.first().copied().unwrap_or_default();
    if matches!(
        reason,
        "denied"
            | "warden_unavailable"
            | "warden_request_failed"
            | "warden_health_failed"
            | "warden_health_invalid"
            | "warden_version_mismatch"
            | "warden_policy_version_missing"
            | "warden_decision_mismatch"
            | "warden_signed_receipt_invalid"
            | "warden_transport_worker_failed"
            | "tradeassembly_policy_bundle_invalid"
            | "extension_identity_mismatch"
    ) {
        response.body["error"]["details"]["reason"] = json!(reason);
    }
    if reason == "warden_request_failed" {
        if let Some(status) = parts
            .get(1)
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|status| (400..600).contains(status))
        {
            response.body["error"]["details"]["httpStatus"] = json!(status);
        }
        if let Some(code) = parts.get(2).filter(|code| {
            matches!(
                **code,
                "unauthorized"
                    | "invalid_request"
                    | "request_policy_mismatch"
                    | "invalid_json"
                    | "unknown"
                    | "required_field_missing"
                    | "non_ijson_number"
                    | "invalid_hash"
                    | "invalid_time_window"
                    | "expired_time_window"
                    | "expired_session"
                    | "authority_exceeds_session"
                    | "raw_secret_payload"
                    | "unexpected_field"
                    | "policy_not_current"
                    | "extension_identity_mismatch"
            )
        }) {
            response.body["error"]["details"]["wardenCode"] = json!(code);
        }
    }
    response
}

fn duplicate_replay_body(mut body: Value, command_id: &str) -> Value {
    if let Value::Object(map) = &mut body {
        map.insert("duplicate".to_string(), json!(true));
        map.entry("commandId".to_string())
            .or_insert_with(|| json!(command_id));
    }
    body
}

#[derive(Clone)]
pub struct TradeAssemblyService {
    db: String,
    runtime: Arc<ServiceRuntime>,
    runtime_manifest: Option<ResolvedRuntimeManifest>,
    invocation_principal: Option<auth::SessionPrincipal>,
    oauth_config: Option<RuntimeConfig>,
    connection_profile: Option<crate::connection_profile::ConnectionProfile>,
    hosted_oauth_actor: Option<String>,
    invocation_actor_kind: &'static str,
    agent_mcp_execution_context: Option<crate::agent_runner::VerifiedAgentMcpExecutionContext>,
}

impl fmt::Debug for TradeAssemblyService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TradeAssemblyService")
            .field("db", &self.db)
            .field("storage_adapter", &self.runtime.storage.adapter_name())
            .finish()
    }
}

impl Default for TradeAssemblyService {
    fn default() -> Self {
        Self::new(".tradeassembly/tradeassembly.db")
    }
}

impl TradeAssemblyService {
    pub(crate) fn cancel_broker_connection_attempt(
        &self,
        instance: &str,
        connection: &str,
    ) -> Result<(), String> {
        plugin_oauth::cancel_attempt(self, instance, connection)
    }
    pub(crate) fn broker_connection_descriptor(&self, instance: &str) -> Result<Value, String> {
        let record = plugin_lifecycle::require_instance(self, instance)
            .map_err(|_| "onboarding_instance_unavailable")?;
        let manifest = plugin_lifecycle::get_manifest(
            self,
            record["pluginRef"]
                .as_str()
                .ok_or("onboarding_plugin_unavailable")?,
        );
        Ok(manifest.body["manifest"]["configuration"]["oauth"].clone())
    }
    pub(crate) fn with_hosted_connection(
        mut self,
        config: RuntimeConfig,
        actor: String,
        profile: Option<crate::connection_profile::ConnectionProfile>,
    ) -> Self {
        self.oauth_config = Some(config);
        self.hosted_oauth_actor = Some(actor);
        self.connection_profile = profile;
        self
    }
    pub fn new(db: impl Into<String>) -> Self {
        Self::local(db)
    }

    /// Bind the trusted Studio origin once at process construction time.
    /// Per-request caller fields never control navigation links.
    pub fn with_studio_base_url(mut self, origin: &str) -> Result<Self, String> {
        validate_studio_origin(origin)?;
        let config = self
            .oauth_config
            .as_mut()
            .ok_or_else(|| "studio_origin_unavailable".to_string())?;
        config.studio_base_url = Some(origin.trim_end_matches('/').to_string());
        Ok(self)
    }

    pub(crate) fn studio_base_url(&self) -> Option<&str> {
        self.oauth_config
            .as_ref()
            .and_then(|config| config.studio_base_url.as_deref())
    }

    pub fn local(db: impl Into<String>) -> Self {
        broker_onboarding::initialize_process_observation();
        let db = db.into();
        let config = RuntimeConfig::local(db.clone());
        let (runtime, manifest) = RuntimeBuilder::new(config.clone())
            .build()
            .expect("built-in local runtime configuration must remain valid");
        let service = Self {
            runtime: Arc::new(runtime),
            db,
            runtime_manifest: Some(manifest),
            oauth_config: Some(config),
            connection_profile: None,
            hosted_oauth_actor: None,
            invocation_principal: None,
            invocation_actor_kind: "user",
            agent_mcp_execution_context: None,
        };
        execution::resume_local_scheduler_workers(&service);
        service
    }

    #[doc(hidden)]
    pub fn test_local(db: impl Into<String>) -> Self {
        let db = db.into();
        assert_test_database_is_owned(&db);
        Self::test_local_with_config(RuntimeConfig::local(db))
    }

    /// Mint a bounded cross-process fixture handoff for an already-owned test
    /// database. This is test-only; ordinary construction never accepts it.
    #[doc(hidden)]
    pub fn test_local_handoff(db: impl Into<String>) -> Result<String, String> {
        let db = db.into();
        let path = test_database_path(&db)?;
        let metadata = std::fs::metadata(&path).map_err(|_| "test_database_unavailable")?;
        let registry = TEST_DATABASE_REGISTRY
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let identity = test_database_identity(&metadata);
        if registry.get(&path) != Some(&identity) {
            return Err("test_database_handoff_requires_owned_fixture".to_string());
        }
        drop(registry);
        let mut token_bytes = [0_u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut token_bytes);
        let token = token_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let record = json!({"dbPath": path.to_string_lossy(), "identity": identity, "tokenHash": format!("sha256:{:x}", Sha256::digest(token.as_bytes()))});
        let sidecar = test_database_handoff_path(&path);
        let bytes = serde_json::to_vec(&record).map_err(|_| "test_database_handoff_failed")?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&sidecar)
            .map_err(|_| "test_database_handoff_failed")?;
        use std::io::Write;
        file.write_all(&bytes)
            .map_err(|_| "test_database_handoff_failed")?;
        Ok(token)
    }

    /// Consume a handoff minted by `test_local_handoff` in another process.
    #[doc(hidden)]
    pub fn test_local_with_handoff(
        db: impl Into<String>,
        token: impl AsRef<str>,
    ) -> Result<Self, String> {
        let db = db.into();
        let path = test_database_path(&db)?;
        let metadata = std::fs::metadata(&path).map_err(|_| "test_database_unavailable")?;
        let sidecar = test_database_handoff_path(&path);
        let sidecar_metadata =
            std::fs::symlink_metadata(&sidecar).map_err(|_| "test_database_handoff_invalid")?;
        if !sidecar_metadata.file_type().is_file() || sidecar_metadata.len() > 4096 {
            return Err("test_database_handoff_invalid".to_string());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if sidecar_metadata.permissions().mode() & 0o077 != 0 {
                return Err("test_database_handoff_invalid".to_string());
            }
        }
        let raw = std::fs::read(&sidecar).map_err(|_| "test_database_handoff_invalid")?;
        let record: Value =
            serde_json::from_slice(&raw).map_err(|_| "test_database_handoff_invalid")?;
        let expected = format!("sha256:{:x}", Sha256::digest(token.as_ref().as_bytes()));
        if record["dbPath"] != path.to_string_lossy().to_string()
            || record["tokenHash"] != expected
            || record["identity"] != json!(test_database_identity(&metadata))
        {
            return Err("test_database_handoff_invalid".to_string());
        }
        let mut registry = TEST_DATABASE_REGISTRY
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry.insert(path, test_database_identity(&metadata));
        drop(registry);
        Ok(Self::test_local(db))
    }

    #[doc(hidden)]
    pub fn test_local_with_plugin_local_egress(db: impl Into<String>) -> Self {
        let db = db.into();
        assert_test_database_is_owned(&db);
        let mut config = RuntimeConfig::local(db);
        config.plugin_sandbox_allow_local_egress = true;
        Self::test_local_with_config(config)
    }

    fn test_local_with_config(config: RuntimeConfig) -> Self {
        broker_onboarding::initialize_process_observation();
        let mut config = config;
        config.oidc_profile = "oidc_test".to_string();
        config.oidc_issuer = "http://127.0.0.1:8080/realms/tradeassembly-dev".to_string();
        config.oidc_audience = "tradeassembly-local".to_string();
        config.oidc_client_id = "tradeassembly-local".to_string();
        let db = config
            .database_path
            .clone()
            .expect("test local runtime requires a database path");
        // Test packages can contain durable controlled-sink state. Never share
        // the production-default artifact store across isolated test databases.
        config.artifact_root = Some(
            std::path::Path::new(&db)
                .with_extension("test-artifacts")
                .to_string_lossy()
                .into_owned(),
        );
        let (runtime, manifest) = RuntimeBuilder::new(config.clone())
            .with_finance_authority(Arc::new(crate::finance_authority::TestFinanceAuthority))
            .build()
            .expect("test local runtime configuration must remain valid");
        let service = Self {
            runtime: Arc::new(runtime),
            db,
            runtime_manifest: Some(manifest),
            oauth_config: Some(config),
            connection_profile: None,
            hosted_oauth_actor: None,
            invocation_principal: None,
            invocation_actor_kind: "user",
            agent_mcp_execution_context: None,
        };
        execution::resume_local_scheduler_workers(&service);
        service
    }

    pub fn from_config(config: RuntimeConfig) -> Result<Self, String> {
        let service = Self::from_config_without_local_scheduler_resume(config)?;
        service.resume_local_scheduler_workers();
        Ok(service)
    }

    pub(crate) fn from_config_without_local_scheduler_resume(
        config: RuntimeConfig,
    ) -> Result<Self, String> {
        broker_onboarding::initialize_process_observation();
        if let Some(origin) = config.studio_base_url.as_deref() {
            validate_studio_origin(origin)?;
        }
        let db = config
            .database_path
            .clone()
            .unwrap_or_else(|| config.profile.as_str().to_string());
        let (runtime, manifest) = RuntimeBuilder::new(config.clone()).build()?;
        Ok(Self {
            db,
            runtime: Arc::new(runtime),
            runtime_manifest: Some(manifest),
            oauth_config: Some(config),
            connection_profile: None,
            hosted_oauth_actor: None,
            invocation_principal: None,
            invocation_actor_kind: "user",
            agent_mcp_execution_context: None,
        })
    }

    pub(crate) fn resume_local_scheduler_workers(&self) {
        if self
            .runtime_manifest
            .as_ref()
            .is_some_and(|manifest| manifest.profile == "local")
        {
            execution::resume_local_scheduler_workers(self);
        }
    }

    pub fn from_runtime(db: impl Into<String>, runtime: ServiceRuntime) -> Self {
        broker_onboarding::initialize_process_observation();
        Self {
            db: db.into(),
            runtime: Arc::new(runtime),
            runtime_manifest: None,
            oauth_config: None,
            connection_profile: None,
            hosted_oauth_actor: None,
            invocation_principal: None,
            invocation_actor_kind: "user",
            agent_mcp_execution_context: None,
        }
    }

    pub fn for_authenticated_invocation(
        &self,
        provider: impl Into<String>,
        stable_identity_id: impl Into<String>,
        email: Option<String>,
        display_name: Option<String>,
    ) -> Self {
        let mut service = self.clone();
        let provider = provider.into();
        service.invocation_principal = Some(auth::SessionPrincipal {
            provider: provider.clone(),
            issuer: provider,
            subject: stable_identity_id.into(),
            email,
            email_verified: None,
            name: display_name,
            picture: None,
        });
        service.invocation_actor_kind = "user";
        service.project_journal_owner();
        service.claim_local_seed_strategies();
        service.claim_local_seed_plugin_instances();
        service
    }

    /// Attaches only a capability that was resolved from the durable runner
    /// state. The capability type has no public constructor and is rechecked
    /// at every MCP call; this does not alter generic local MCP behavior.
    #[doc(hidden)]
    pub fn with_verified_agent_mcp_execution_context(
        &self,
        context: crate::agent_runner::VerifiedAgentMcpExecutionContext,
    ) -> Self {
        let mut service = self.clone();
        service.agent_mcp_execution_context = Some(context);
        service
    }

    pub(crate) fn install_plugin_package_for_local_setup(
        &self,
        body: Value,
    ) -> Result<ServiceResponse, String> {
        if self
            .runtime_manifest
            .as_ref()
            .is_none_or(|manifest| manifest.profile != "local")
        {
            return Err("local setup identity requires the local runtime profile".to_string());
        }
        let mut service = self.clone();
        service.invocation_principal = Some(auth::SessionPrincipal {
            provider: "tradeassembly_local_setup".to_string(),
            issuer: "tradeassembly://local-setup".to_string(),
            subject: "tradeassembly.local_setup".to_string(),
            email: None,
            email_verified: None,
            name: Some("TradeAssembly local setup".to_string()),
            picture: None,
        });
        service.invocation_actor_kind = "application";
        service.project_journal_owner();
        Ok(service.handle_http_from_source("local_setup", "POST", "/plugins/packages", body))
    }

    pub fn register_verified_invocation_identity(
        &self,
        session_ref: &str,
        claims: &crate::ports::IdentityClaims,
        now_ms: i64,
    ) -> Result<(), String> {
        self.runtime
            .identity
            .store_verified_session(session_ref, claims)?;
        let resolved = self.runtime.identity.resolve(session_ref, now_ms)?;
        if resolved != *claims {
            return Err("verified invocation identity projection mismatch".to_string());
        }
        Ok(())
    }

    pub fn db(&self) -> &str {
        &self.db
    }

    pub fn runtime(&self) -> Arc<ServiceRuntime> {
        Arc::clone(&self.runtime)
    }

    pub(crate) fn responsible_human_identity(&self) -> Option<(&str, &str)> {
        if self.invocation_actor_kind != "user" {
            return None;
        }
        self.invocation_principal
            .as_ref()
            .map(|principal| (principal.issuer.as_str(), principal.subject.as_str()))
    }

    fn project_journal_owner(&mut self) {
        let mut runtime = self.runtime.as_ref().clone();
        runtime.journal_owner = self.invocation_owner();
        self.runtime = Arc::new(runtime);
    }

    pub(crate) fn agent_mcp_execution_context(
        &self,
    ) -> Option<&crate::agent_runner::VerifiedAgentMcpExecutionContext> {
        self.agent_mcp_execution_context.as_ref()
    }

    pub fn runtime_manifest(&self) -> Option<&ResolvedRuntimeManifest> {
        self.runtime_manifest.as_ref()
    }

    pub fn control_plane_commands(&self) -> Vec<Value> {
        control_plane::list_commands(self.runtime.storage.as_ref())
            .unwrap_or_default()
            .into_iter()
            .map(|record| serde_json::to_value(record).unwrap_or_else(|_| json!({})))
            .collect()
    }

    fn complete_prepared_command_with_body(
        &self,
        envelope: &Option<control_plane::ControlPlaneCommandEnvelope>,
        status: u16,
        body: Option<Value>,
    ) -> Result<(), String> {
        if let Some(envelope) = envelope {
            control_plane::complete_command_with_body(
                self.runtime.storage.as_ref(),
                self.runtime.finance_authority.as_ref(),
                envelope,
                status,
                body,
            )?;
        }
        Ok(())
    }

    fn complete_graphql_command_or_error(
        &self,
        envelope: &Option<control_plane::ControlPlaneCommandEnvelope>,
        status: u16,
        response: Value,
    ) -> Value {
        if status == 503 {
            return response;
        }
        if self
            .complete_prepared_command_with_body(envelope, status, Some(response.clone()))
            .is_err()
        {
            json!({"errors": [{"message": "control_plane_persistence_failed"}]})
        } else {
            response
        }
    }

    fn complete_mcp_command_or_error(
        &self,
        name: &str,
        envelope: &Option<control_plane::ControlPlaneCommandEnvelope>,
        status: u16,
        response: Value,
    ) -> Value {
        if status == 503 {
            return response;
        }
        if self
            .complete_prepared_command_with_body(envelope, status, Some(response.clone()))
            .is_err()
        {
            mcp::tool_error(
                name,
                "control_plane_persistence_failed",
                "MCP command result could not be durably recorded.",
                None,
            )
        } else {
            response
        }
    }

    pub fn record_side_effect(
        &self,
        event_type: &str,
        payload: Value,
        authority: Option<AuthorityContext>,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<String, String> {
        let authority = authority
            .ok_or_else(|| "authority context is required for side effects".to_string())?;
        let idempotency_key = idempotency_key
            .ok_or_else(|| "idempotency key is required for side effects".to_string())?;
        let context = SideEffectContext::new(authority, idempotency_key);
        self.runtime
            .storage
            .put_json("side_effects", event_type, payload.clone(), &context)?;
        self.runtime
            .record_side_effect(event_type, payload, &context)
    }

    pub fn handle_http(&self, method: &str, path: &str, body: Value) -> ServiceResponse {
        self.handle_http_from_source("http", method, path, body)
    }

    pub fn set_sightline_selection_for_authenticated_user(
        &self,
        session: &auth::SessionValidationResult,
        body: Value,
    ) -> Value {
        let Some(principal) = session.principal.as_ref().filter(|_| session.authenticated) else {
            return json!({
                "ok": false,
                "body": {"selection": Value::Null, "context": Value::Null},
                "error": {"code": "sightline_user_authority_required"},
            });
        };
        sightline::set_selection(self, body, sightline::user_actor_from_principal(principal))
    }

    pub fn share_sightline_selection_for_authenticated_user(
        &self,
        session: &auth::SessionValidationResult,
        body: Value,
    ) -> Value {
        let Some(principal) = session.principal.as_ref().filter(|_| session.authenticated) else {
            return json!({
                "ok": false,
                "body": {"selection": Value::Null, "context": Value::Null},
                "error": {"code": "sightline_user_authority_required"},
            });
        };
        sightline::share_selection(self, body, sightline::user_actor_from_principal(principal))
    }

    pub fn handle_studio_graphql(
        &self,
        bearer_credential: Option<&str>,
        request: Value,
    ) -> ServiceResponse {
        let assertion = request
            .get("variables")
            .and_then(|variables| variables.get("_studioSession"))
            .cloned()
            .and_then(|value| {
                serde_json::from_value::<crate::ports::TrustedStudioSession>(value).ok()
            });
        let (service, session) =
            match self.authenticated_studio_invocation(bearer_credential, assertion) {
                Ok(value) => value,
                Err(response) => return response,
            };
        let request = bind_authenticated_studio_request(request, &session);
        ServiceResponse::ok(service.execute_graphql_with_session(request, Some(&session)))
    }

    pub fn handle_studio_http(
        &self,
        bearer_credential: Option<&str>,
        assertion: Option<crate::ports::TrustedStudioSession>,
        method: &str,
        path: &str,
        body: Value,
    ) -> ServiceResponse {
        match self.authenticated_studio_invocation(bearer_credential, assertion) {
            Ok((service, _)) => service.handle_http_from_source("studio", method, path, body),
            Err(response) => response,
        }
    }

    fn authenticated_studio_invocation(
        &self,
        bearer_credential: Option<&str>,
        assertion: Option<crate::ports::TrustedStudioSession>,
    ) -> Result<(Self, auth::SessionValidationResult), ServiceResponse> {
        let Some(bearer_credential) = bearer_credential.filter(|value| !value.trim().is_empty())
        else {
            return Err(ServiceResponse::unauthorized(
                "studio_transport_authentication_required",
            ));
        };
        let Some(assertion) = assertion else {
            return Err(ServiceResponse::unauthorized(
                "studio_oidc_session_required",
            ));
        };
        let claims = match self.runtime.identity.authenticate_studio(
            bearer_credential,
            &assertion,
            self.runtime.clock.now_ms(),
        ) {
            Ok(claims) => claims,
            Err(_) => return Err(ServiceResponse::unauthorized("studio_identity_invalid")),
        };
        let principal = auth::SessionPrincipal {
            provider: "oidc".to_string(),
            issuer: claims.issuer,
            subject: claims.subject,
            email: assertion.email,
            email_verified: None,
            name: Some(assertion.display_name),
            picture: None,
        };
        let session = auth::SessionValidationResult {
            authenticated: true,
            reason: "ok".to_string(),
            principal: Some(principal.clone()),
        };
        let mut service = self.clone();
        service.invocation_principal = Some(principal);
        service.project_journal_owner();
        service.claim_local_seed_strategies();
        service.claim_local_seed_plugin_instances();
        Ok((service, session))
    }

    pub fn handle_http_from_source(
        &self,
        source_interface: &str,
        method: &str,
        path: &str,
        body: Value,
    ) -> ServiceResponse {
        if method.eq_ignore_ascii_case("POST")
            && normalize_path(path) == "/orders/broker-recovery"
            && !broker_recovery::valid_request(&body)
        {
            return ServiceResponse::error(400, "broker_recovery_request_invalid", false);
        }
        // Configuration authoring records user intent; it does not authorize an
        // order. Do not silently rewrite a requested Live config to Paper.
        let preserve_config_mode = method.eq_ignore_ascii_case("POST")
            && normalize_path(path) == "/product/strategy-execution-configs/save";
        let body = self
            .invocation_principal
            .as_ref()
            .map_or(body.clone(), |principal| {
                bind_authenticated_invocation_value(
                    body,
                    principal,
                    source_interface,
                    self.invocation_actor_kind,
                    preserve_config_mode,
                    None,
                )
            });
        let normalized_method = method.to_ascii_uppercase();
        let normalized_path = normalize_path(path);
        if let Some(response) =
            object_authorization::authorize_private_request(self, method, path, &body)
        {
            return response;
        }
        if normalized_method == "POST"
            && matches!(normalized_path.as_str(), "/demo/reset" | "/demo/reset-btc")
        {
            return self.dispatch_http(method, path, body);
        }
        if normalized_method == "POST" && normalized_path == "/graphql" {
            return self.dispatch_http(method, path, body);
        }
        if normalized_method == "POST"
            && matches!(
                normalized_path.as_str(),
                "/product/strategies/research-notebook/list"
                    | "/product/strategies/research-notebook/inspect"
                    | "/product/strategies/research-notebook/export"
                    | "/product/strategies/research-notebook/replay"
            )
        {
            // These are integrity-checked reads. They do not create a command,
            // journal event, or cached control-plane response.
            return self.dispatch_http(method, path, body);
        }

        let body = self.enrich_control_plane_body(&normalized_path, body);
        let envelope = match control_plane::http_envelope(source_interface, method, path, &body) {
            Ok(envelope) => inherit_trusted_correlation(envelope, &body),
            Err(_) => return ServiceResponse::bad_request("invalid_command_envelope"),
        };
        match control_plane::prepare_command(
            self.runtime.storage.as_ref(),
            self.runtime.finance_authority.as_ref(),
            envelope.clone(),
        ) {
            Ok(record) if record.duplicate => {
                return record
                    .response_status
                    .map(|status| {
                        record.response_body.clone().map_or_else(
                            || ServiceResponse::duplicate(status, &record.envelope.command_id),
                            |body| {
                                ServiceResponse::duplicate_with_body(
                                    status,
                                    &record.envelope.command_id,
                                    body,
                                )
                            },
                        )
                    })
                    .unwrap_or_else(|| ServiceResponse::conflict("idempotency_in_progress"));
            }
            Ok(_) => {}
            Err(error) if error == "idempotency_conflict" => {
                return ServiceResponse::conflict("idempotency_conflict");
            }
            Err(error) if error.starts_with("finance_authority:") => {
                if normalized_path == "/product/strategy-execution-activations/activate" {
                    return ServiceResponse::forbidden_with_details(
                        "finance_authority_denied",
                        execution::activation_blocked_preflight(self, body.clone()),
                    );
                }
                return finance_authority_denied_response(&error);
            }
            Err(error) if error.starts_with("stale_sequence") => {
                return ServiceResponse::conflict("stale_sequence");
            }
            Err(_) => return ServiceResponse::internal_error("control plane persistence failed"),
        }
        let dispatch_body = with_control_plane_metadata(body, &envelope);
        let response = self.dispatch_http(method, path, dispatch_body);
        if response.status == 503 && response.body["error"]["retryable"].as_bool() == Some(true) {
            return response;
        }
        if control_plane::complete_command_with_body(
            self.runtime.storage.as_ref(),
            self.runtime.finance_authority.as_ref(),
            &envelope,
            response.status,
            Some(response.body.clone()),
        )
        .is_err()
        {
            return ServiceResponse::internal_error("control plane persistence failed");
        }
        response
    }

    fn enrich_graphql_request(&self, request: &Value, operation: &str) -> Value {
        let Some(path) = graphql_control_route(operation) else {
            return request.clone();
        };
        let mut request = request.clone();
        let mut variables = request
            .get("variables")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if operation == "CreateDatasetIngestion" {
            let command = variables
                .get("request")
                .cloned()
                .unwrap_or_else(|| json!({}));
            variables["request"] = self.enrich_control_plane_body(path, command);
        } else {
            variables = self.enrich_control_plane_body(path, variables);
        }
        if let Value::Object(map) = &mut request {
            map.insert("variables".to_string(), variables);
        }
        request
    }

    fn enrich_mcp_arguments(&self, name: &str, arguments: Value) -> Value {
        let mut arguments =
            self.enrich_control_plane_body(mcp_control_route(name).unwrap_or_default(), arguments);
        if name.starts_with("tradeassembly.sightline.") {
            if let Value::Object(map) = &mut arguments {
                let mut actor = map
                    .get("actor")
                    .cloned()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({"id": "tradeassembly.mcp_agent"}));
                let actor_map = actor.as_object_mut().expect("actor is an object");
                actor_map.insert("kind".to_string(), json!("agent"));
                actor_map.insert("actor_type".to_string(), json!("agent"));
                actor_map.insert("actorType".to_string(), json!("agent"));
                map.insert("actor".to_string(), actor);
            }
        }
        arguments
    }

    fn enrich_control_plane_body(&self, path: &str, mut body: Value) -> Value {
        if let Value::Object(map) = &mut body {
            for key in [
                "correlationId",
                "correlation_id",
                "controlPlaneCorrelationId",
            ] {
                map.remove(key);
            }
        }
        if path.starts_with("/product/sightline/") {
            return enrich_sightline_control_plane_body(body);
        }
        if path.starts_with("/product/strategies/research-notebook") {
            if let Value::Object(map) = &mut body {
                let operation = path.rsplit('/').next().unwrap_or("compose");
                map.entry("researchNotebookOperation".to_string())
                    .or_insert_with(|| json!(operation));
            }
        }
        if let Value::Object(map) = &mut body {
            if !map.contains_key("instanceRef") {
                if let Some(instance_ref) = plugin_instance_ref_from_path(path) {
                    map.insert("instanceRef".to_string(), json!(instance_ref));
                }
            }
        }
        if !control_plane_needs_execution_context(path) {
            return body;
        }
        let Value::Object(map) = &mut body else {
            return body;
        };
        if !map.contains_key("activationId") {
            if let Some(activation_id) = execution_activation_id_from_path(path) {
                map.insert("activationId".to_string(), json!(activation_id));
            }
        }
        if let Some(config_id) = map_string(map, &["configId", "config_id"]) {
            if let Ok(Some(config)) = self
                .runtime
                .storage
                .get_json("execution_configs", &config_id)
            {
                copy_execution_context(map, &config);
            }
        }
        if path != "/product/strategy-execution-activations/activate" {
            if let Some(activation_id) = map_string(map, &["activationId", "activation_id"]) {
                let run_id = format!("run_{activation_id}");
                if let Ok(Some(run)) = self.runtime.storage.get_json("execution_runs", &run_id) {
                    copy_execution_context(map, &run);
                }
            }
        }
        if !map.contains_key("accountMode") {
            if let Some(mode) = map_string(map, &["mode", "accountMode", "account_mode"]) {
                map.insert("accountMode".to_string(), json!(mode));
            }
        }
        body
    }

    fn dispatch_http(&self, method: &str, path: &str, body: Value) -> ServiceResponse {
        let normalized_method = method.to_ascii_uppercase();
        let normalized_path = normalize_path(path);
        if let Some(response) =
            object_authorization::authorize_private_request(self, method, path, &body)
        {
            return response;
        }
        match (normalized_method.as_str(), normalized_path.as_str()) {
            ("POST", "/product/live-mandates/issue") => live_authorization::issue(self, body),
            ("POST", "/product/live-mandates/revoke") => live_authorization::revoke(self, body),
            ("POST", "/product/live-mandates/status") => live_authorization::status(self, body),
            ("POST", "/plugins/oauth/start") => plugin_oauth::start(self, body),
            ("POST", "/plugins/oauth/status") => plugin_oauth::status(self, body),
            ("POST", "/plugins/oauth/disconnect") => plugin_oauth::disconnect(self, body),
            ("GET", "/health") => ServiceResponse::ok(json!({
                "status": "ok",
                "service": "tradeassembly",
                "runtime": "rust",
                "legalBoundary": LEGAL_BOUNDARY,
            })),
            ("GET", "/ready") => ServiceResponse::ok(json!({"status": "ready"})),
            ("GET", "/workspace/shell") => ServiceResponse::ok(self.workspace_shell()),
            ("GET", "/workspace") => ServiceResponse::ok(self.workspace()),
            ("POST", "/graphql") => ServiceResponse::ok(self.execute_graphql(body)),
            ("POST", "/demo/btc-exit") => ServiceResponse::ok(self.seed_btc_demo()),
            ("POST", "/demo/reset") | ("POST", "/demo/reset-btc") => {
                match strategy::reset_local_demo(self) {
                    Ok(mut namespaces) => match sightline::reset(self) {
                        Ok(sightline_namespaces) => {
                            namespaces.extend(sightline_namespaces);
                            ServiceResponse::ok(json!({
                            "status": "reset",
                            "db": self.db,
                            "resetApplied": true,
                            "namespaces": namespaces,
                            }))
                        }
                        Err(error) => ServiceResponse::internal_error(&error),
                    },
                    Err(error) => ServiceResponse::internal_error(&error),
                }
            }
            ("GET", "/strategies") => ServiceResponse::ok(json!(self.strategies())),
            ("POST", "/strategies") | ("POST", "/product/strategies/create") => {
                let result = self.create_strategy(body);
                if result["ok"].as_bool() == Some(false) {
                    ServiceResponse::bad_request_with_details(
                        "strategy_draft_invalid",
                        json!({
                            "validation": result["body"]["validation"],
                            "persistenceViolations": result["body"]["persistenceViolations"],
                        }),
                    )
                } else {
                    ServiceResponse::created(result)
                }
            }
            ("POST", "/strategies/draft") | ("POST", "/product/strategies/save-draft") => {
                ServiceResponse::ok(self.save_builder_draft(body))
            }
            ("POST", "/product/strategies/semantic-selection") => {
                ServiceResponse::ok(self.select_strategy_semantic_node(body))
            }
            ("POST", "/product/strategies/proposals/create") => {
                ServiceResponse::ok(self.propose_strategy_patch(body))
            }
            ("POST", "/product/strategies/proposals/review")
            | ("POST", "/product/strategies/proposals/apply") => {
                ServiceResponse::ok(self.review_strategy_patch(body))
            }
            ("GET", "/product/sightline/session-state") => {
                ServiceResponse::ok(sightline::session_state(self, body))
            }
            ("POST", "/product/sightline/surfaces/register") => {
                ServiceResponse::ok(sightline::register_surface(self, body))
            }
            ("POST", "/product/sightline/strategy-editor/publish") => {
                ServiceResponse::ok(sightline::publish_strategy_editor_surface(self, body))
            }
            ("POST", "/product/sightline/studio-surface/publish") => {
                ServiceResponse::ok(sightline::publish_studio_surface(self, body))
            }
            ("POST", "/product/sightline/selections/set") => {
                ServiceResponse::forbidden("sightline_user_authority_required")
            }
            ("POST", "/product/sightline/selections/share") => {
                ServiceResponse::forbidden("sightline_user_authority_required")
            }
            ("POST", "/product/sightline/context/get") => {
                ServiceResponse::ok(sightline::get_context(self, body))
            }
            ("POST", "/product/sightline/agents/connect") => {
                ServiceResponse::ok(sightline::connect_agent(self, body))
            }
            ("POST", "/product/sightline/presence/update") => {
                ServiceResponse::ok(sightline::update_presence(self, body))
            }
            ("POST", "/product/sightline/navigation/request") => {
                ServiceResponse::ok(sightline::request_navigation(self, body))
            }
            ("POST", "/product/sightline/nodes/focus") => {
                ServiceResponse::ok(sightline::focus_node(self, body))
            }
            ("POST", "/product/sightline/nodes/highlight") => {
                ServiceResponse::ok(sightline::highlight_node(self, body))
            }
            ("POST", "/product/sightline/proposals/create") => {
                ServiceResponse::ok(sightline::create_proposal(self, body))
            }
            ("POST", "/product/sightline/approvals/request") => {
                ServiceResponse::ok(sightline::request_approval(self, body))
            }
            ("GET", "/product/sightline/events") => {
                ServiceResponse::ok(sightline::list_events(self, body))
            }
            ("GET", path) if path.starts_with("/strategies/") => {
                ServiceResponse::ok(self.strategy_detail(path_strategy_id(path)))
            }
            ("POST", "/strategy/contracts/validate") => {
                let payload = strip_strategy_transport_metadata(body);
                let report = spec::validate_strategy_spec_report(&payload);
                if report.valid {
                    let mut result = contracts::validation_result(true, json!([]));
                    result["report"] = json!(report);
                    ServiceResponse::ok(api_result(result))
                } else {
                    ServiceResponse::bad_request_with_details(
                        "validation_failed",
                        json!({"report": report}),
                    )
                }
            }
            ("GET", "/strategy/contracts/schema") => {
                ServiceResponse::ok(spec::strategy_spec_schema())
            }
            ("GET", "/strategy/contracts/stages") => ServiceResponse::ok(json!({
                "stages": ["inputs", "universe", "evaluate", "intent", "sizing", "risk", "exit_plan", "order_strategy"],
                "schemaVersion": "tradeassembly.strategy_contract_stages.v1",
            })),
            ("GET", "/strategy/contracts/substeps") => ServiceResponse::ok(json!({
                "substeps": ["load_market_data", "evaluate_rules", "prepare_order_intent", "reserve_risk", "journal_evidence"],
                "schemaVersion": "tradeassembly.strategy_contract_substeps.v1",
            })),
            ("GET", "/strategy/contracts/indicator-schema") => ServiceResponse::ok(json!({
                "indicators": [{"id": "sma", "inputs": ["close", "window"]}, {"id": "rsi", "inputs": ["close", "window"]}],
                "schemaVersion": "tradeassembly.indicator_schema.v1",
            })),
            ("POST", "/strategy/contracts/envelope-preview")
            | ("POST", "/strategy/contracts/compile-preview") => compile_strategy_preview(body),
            ("POST", "/strategy/contracts/compatibility:resolve") => ServiceResponse::ok(json!({
                "ok": true,
                "compatible": true,
                "evidence": evidence("strategy.contract.compatibility_resolved"),
            })),
            ("GET", "/providers") | ("POST", "/product/providers/list") => {
                ServiceResponse::ok(providers::connect_workspace(self))
            }
            ("GET", "/providers/policy") => {
                ServiceResponse::ok(json!({"allowlist": [], "pins": []}))
            }
            ("PUT", "/providers/policy") => {
                ServiceResponse::ok(json!({"ok": true, "policy": body}))
            }
            ("GET", "/providers/broker-contracts") => {
                ServiceResponse::ok(json!([{"providerRef": "sim", "paper": true}]))
            }
            ("POST", "/providers/broker-conformance") => ServiceResponse::ok(
                crate::conformance::broker_conformance(&["sim"], true, false),
            ),
            ("POST", "/entitlements/deny") => {
                ServiceResponse::ok(providers::deny_entitlement(self, body))
            }
            ("GET", "/entitlements") => ServiceResponse::ok(providers::list_entitlements(self)),
            ("DELETE", path) if path.starts_with("/entitlements/") => {
                ServiceResponse::ok(providers::revoke_entitlement(self, path_id(path)))
            }
            ("POST", "/entitlements/grant") => {
                ServiceResponse::ok(providers::grant_entitlement(self, body))
            }
            ("POST", "/plugins/capabilities/resolve")
            | ("POST", "/capabilities/resolve")
            | ("POST", "/strategy/contracts/capability:resolve") => ServiceResponse::ok(
                providers::resolve_capability(self, providers::requirement_from_body(&body)),
            ),
            ("POST", "/capabilities/graph/resolve") => {
                ServiceResponse::ok(capability_graph::resolve(self, body))
            }
            ("POST", "/capability-graph-revisions") => {
                ServiceResponse::ok(capability_graph::save_revision(self, body))
            }
            ("POST", "/capability-graph-revisions/currentness") => {
                ServiceResponse::ok(capability_graph::check_current(self, body))
            }
            ("GET", path) if path.starts_with("/capability-graph-revisions/") => {
                ServiceResponse::ok(capability_graph::get_revision(self, path_id(path)))
            }
            ("POST", "/product/providers/update-instance") => {
                let requested_ref = provider_ref_from(&body);
                let instance_ref =
                    plugin_lifecycle::compatibility_instance_ref(self, &requested_ref);
                let enabled = body.get("enabled").and_then(Value::as_bool).unwrap_or(
                    !body
                        .get("disabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                );
                plugin_lifecycle::compatibility_instance_response(plugin_lifecycle::set_enabled(
                    self,
                    &instance_ref,
                    enabled,
                ))
            }
            ("POST", "/product/providers/enable-instance") => {
                let requested_ref = provider_ref_from(&body);
                let instance_ref =
                    plugin_lifecycle::compatibility_instance_ref(self, &requested_ref);
                plugin_lifecycle::compatibility_instance_response(plugin_lifecycle::set_enabled(
                    self,
                    &instance_ref,
                    true,
                ))
            }
            ("POST", "/product/providers/disable-instance") => {
                let requested_ref = provider_ref_from(&body);
                let instance_ref =
                    plugin_lifecycle::compatibility_instance_ref(self, &requested_ref);
                plugin_lifecycle::compatibility_instance_response(plugin_lifecycle::set_enabled(
                    self,
                    &instance_ref,
                    false,
                ))
            }
            ("GET", path) if path.starts_with("/providers/") && path.ends_with("/credentials") => {
                let requested_ref = provider_from_path(path);
                match plugin_lifecycle::active_compatibility_instance_ref(self, requested_ref) {
                    Ok(instance_ref) => ServiceResponse::ok(self.credential_status(&instance_ref)),
                    Err(response) => response,
                }
            }
            ("POST", path) if path.starts_with("/providers/") && path.ends_with("/credentials") => {
                let requested_ref = provider_from_path(path);
                match plugin_lifecycle::active_compatibility_instance_ref(self, requested_ref) {
                    Ok(instance_ref) => ServiceResponse::ok(api_result(credentials::store(
                        self,
                        &instance_ref,
                        body,
                    ))),
                    Err(response) => response,
                }
            }
            ("DELETE", path)
                if path.starts_with("/providers/") && path.ends_with("/credentials") =>
            {
                let requested_ref = provider_from_path(path);
                match plugin_lifecycle::active_compatibility_instance_ref(self, requested_ref) {
                    Ok(instance_ref) => {
                        ServiceResponse::ok(credentials::revoke(self, &instance_ref))
                    }
                    Err(response) => response,
                }
            }
            ("POST", path)
                if path.starts_with("/providers/") && path.ends_with("/credentials/test") =>
            {
                ServiceResponse::ok(api_result(credentials::test(
                    self,
                    provider_from_path(path),
                    body,
                )))
            }
            ("GET", path) if path.contains("/oauth/") => {
                ServiceResponse::ok(credentials::oauth_metadata(provider_from_path(path)))
            }
            ("POST", path) if path.contains("/oauth/") => {
                ServiceResponse::ok(credentials::oauth_metadata(provider_from_path(path)))
            }
            ("DELETE", path) if path.contains("/oauth/") => {
                ServiceResponse::ok(credentials::revoke(self, provider_from_path(path)))
            }
            ("GET", "/plugins/manifests") => {
                ServiceResponse::ok(plugin_lifecycle::list_manifests(self))
            }
            ("POST", "/plugins/manifests") => plugin_lifecycle::install_manifest(self, body),
            ("POST", "/plugins/packages") => plugin_lifecycle::install_package(self, body),
            ("GET", path)
                if path.starts_with("/plugins/manifests/")
                    && path.trim_matches('/').split('/').count() == 3 =>
            {
                plugin_lifecycle::get_manifest(self, plugin_path_ref(path))
            }
            ("GET", "/plugins/instances") => {
                ServiceResponse::ok(plugin_lifecycle::list_instances(self))
            }
            ("POST", "/plugins/instances") => plugin_lifecycle::create_instance(self, body),
            ("GET", path)
                if path.starts_with("/plugins/instances/")
                    && path.trim_matches('/').split('/').count() == 3 =>
            {
                plugin_lifecycle::get_instance(self, plugin_path_ref(path))
            }
            ("PUT", path)
                if path.starts_with("/plugins/instances/") && path.ends_with("/configuration") =>
            {
                plugin_lifecycle::configure_instance(self, plugin_path_ref(path), body)
            }
            ("POST", path)
                if path.starts_with("/plugins/instances/") && path.ends_with(":enable") =>
            {
                plugin_lifecycle::set_enabled(self, plugin_path_ref(path), true)
            }
            ("POST", path)
                if path.starts_with("/plugins/instances/") && path.ends_with(":disable") =>
            {
                plugin_lifecycle::set_enabled(self, plugin_path_ref(path), false)
            }
            ("POST", path)
                if path.starts_with("/plugins/instances/") && path.ends_with(":upgrade") =>
            {
                plugin_lifecycle::upgrade_instance(self, plugin_path_ref(path), body)
            }
            ("POST", path)
                if path.starts_with("/plugins/instances/") && path.ends_with(":rollback") =>
            {
                plugin_lifecycle::rollback_instance(self, plugin_path_ref(path))
            }
            ("DELETE", path)
                if path.starts_with("/plugins/instances/")
                    && path.trim_matches('/').split('/').count() == 3 =>
            {
                plugin_lifecycle::remove_instance(self, plugin_path_ref(path))
            }
            ("GET", path)
                if path.starts_with("/plugins/instances/") && path.ends_with("/credentials") =>
            {
                plugin_lifecycle::credential_status_for(self, plugin_path_ref(path))
            }
            ("POST", path)
                if path.starts_with("/plugins/instances/") && path.ends_with("/credentials") =>
            {
                plugin_lifecycle::store_credentials(self, plugin_path_ref(path), body)
            }
            ("DELETE", path)
                if path.starts_with("/plugins/instances/") && path.ends_with("/credentials") =>
            {
                plugin_lifecycle::revoke_credentials(self, plugin_path_ref(path))
            }
            ("POST", path)
                if path.starts_with("/plugins/instances/") && path.ends_with("/health:refresh") =>
            {
                plugin_lifecycle::refresh_health(self, plugin_path_ref(path))
            }
            ("POST", "/product/plugins/providers") => ServiceResponse::ok(self.plugins_providers()),
            ("POST", path)
                if path.starts_with("/plugins/instances/")
                    && path.contains("/operations/")
                    && path.ends_with(":invoke") =>
            {
                providers::invoke_operation(self, path, body)
            }
            ("POST", path) if path.starts_with("/plugins/") => {
                ServiceResponse::bad_request_with_details(
                    "plugin_route_not_implemented",
                    json!({"path": path, "workspacePrimitive": "plugins"}),
                )
            }
            ("POST", "/strategies/{strategy_id}/backtests") | ("POST", "/backtests") => {
                backtest_lifecycle::create(self, body)
            }
            ("POST", path) if path.starts_with("/strategies/") && path.ends_with("/backtests") => {
                backtest_lifecycle::create(self, body)
            }
            ("POST", "/product/strategies/backtests/run") => backtest_lifecycle::create(self, body),
            // Historical Studio clients use this route; it is an exact alias
            // for the durable backtest lifecycle and therefore preserves its
            // status, validation, idempotency, and queued response.
            ("POST", "/product/strategies/research-runs/create") => {
                backtest_lifecycle::create(self, body)
            }
            ("POST", "/dataset-ingestions") => {
                dataset_ingestion_response(dataset_ingestion::create(self, body), 201)
            }
            ("GET", "/dataset-ingestions") => {
                ServiceResponse::ok(json!({"ingestions": dataset_ingestion::list(self)}))
            }
            ("GET", path) if path.starts_with("/dataset-ingestions/") => {
                dataset_ingestion_response(
                    dataset_ingestion::get(self, dataset_ingestion_path_id(path)),
                    200,
                )
            }
            ("POST", path)
                if path.starts_with("/dataset-ingestions/") && path.ends_with("/cancel") =>
            {
                dataset_ingestion_response(
                    dataset_ingestion::cancel(self, dataset_ingestion_path_id(path), &body),
                    200,
                )
            }
            ("POST", path)
                if path.starts_with("/dataset-ingestions/") && path.ends_with("/verify") =>
            {
                dataset_ingestion_response(
                    dataset_ingestion::verify(self, dataset_ingestion_path_id(path)),
                    200,
                )
            }
            ("GET", "/backtests") => backtest_lifecycle::list(self),
            ("POST", "/backtests:process") => backtest_lifecycle::process_request(
                self,
                body.get("worker")
                    .and_then(Value::as_str)
                    .unwrap_or("local-backtest-worker"),
                body.get("runId")
                    .or_else(|| body.get("run_id"))
                    .and_then(Value::as_str),
            ),
            ("GET", path)
                if path.starts_with("/backtests/")
                    && path.trim_matches('/').split('/').count() == 2 =>
            {
                backtest_lifecycle::get(self, path_id(path))
            }
            ("POST", path) if path.starts_with("/backtests/") && path.ends_with("/cancel") => {
                backtest_lifecycle::cancel(self, path_id(path), body)
            }
            ("POST", path) if path.starts_with("/backtests/") && path.ends_with("/retry") => {
                backtest_lifecycle::retry(self, path_id(path), body)
            }
            ("POST", path) if path.starts_with("/backtests/") && path.ends_with("/replay") => {
                backtest_lifecycle::replay(self, path_id(path))
            }
            ("POST", path) if path.starts_with("/backtests/") && path.ends_with("/export") => {
                backtest_lifecycle::export(
                    self,
                    path_id(path),
                    body.get("exportKind")
                        .or_else(|| body.get("export_kind"))
                        .and_then(Value::as_str)
                        .unwrap_or("reportJson"),
                )
            }
            ("GET", path) if path.starts_with("/backtests/") && path.ends_with("/report") => {
                backtest_lifecycle::report(self, path_id(path))
            }
            ("POST", "/product/strategies/backtests/export") => backtest_lifecycle::export(
                self,
                body.get("backtestId")
                    .or_else(|| body.get("backtest_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                body.get("exportKind")
                    .or_else(|| body.get("export_kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("reportJson"),
            ),
            ("POST", "/robustness-runs") => robustness::create(self, body),
            ("POST", "/robustness-runs:process") => robustness::process(
                self,
                body.get("worker")
                    .and_then(Value::as_str)
                    .unwrap_or("local-robustness-worker"),
            ),
            ("GET", "/robustness-runs") => robustness::list(self),
            ("GET", path) if path.starts_with("/robustness-runs/") && path.ends_with("/report") => {
                robustness::report(self, path_id(path))
            }
            ("POST", path)
                if path.starts_with("/robustness-runs/") && path.ends_with("/replay") =>
            {
                robustness::replay(self, path_id(path))
            }
            ("POST", path)
                if path.starts_with("/robustness-runs/") && path.ends_with("/export") =>
            {
                robustness::export(
                    self,
                    path_id(path),
                    body.get("format").and_then(Value::as_str).unwrap_or("json"),
                )
            }
            ("GET", path)
                if path.starts_with("/robustness-runs/")
                    && path.trim_matches('/').split('/').count() == 2 =>
            {
                robustness::get(self, path_id(path))
            }
            ("POST", path)
                if path.starts_with("/robustness-runs/") && path.ends_with("/cancel") =>
            {
                robustness::cancel(self, path_id(path), body)
            }
            ("POST", path) if path.starts_with("/robustness-runs/") && path.ends_with("/retry") => {
                robustness::retry(self, path_id(path), body)
            }
            ("POST", "/derivatives-analyses") => derivatives::create(self, body),
            ("GET", "/derivatives-analyses") => derivatives::list(self),
            ("GET", path)
                if path.starts_with("/derivatives-analyses/") && path.contains("/exports/") =>
            {
                let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
                if parts.len() == 4 {
                    derivatives::export(self, parts[1], parts[3])
                } else {
                    ServiceResponse::bad_request("derivatives_export_path_invalid")
                }
            }
            ("GET", path)
                if path.starts_with("/derivatives-analyses/")
                    && path.trim_matches('/').split('/').count() == 2 =>
            {
                derivatives::get(self, path_id(path))
            }
            ("POST", "/research-comparisons") => comparisons::create(self, body),
            ("GET", "/research-comparisons") => comparisons::list(self),
            ("GET", path)
                if path.starts_with("/research-comparisons/") && path.contains("/exports/") =>
            {
                let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
                if parts.len() == 4 {
                    comparisons::export(self, parts[1], parts[3])
                } else {
                    ServiceResponse::bad_request("comparison_export_path_invalid")
                }
            }
            ("GET", path)
                if path.starts_with("/research-comparisons/")
                    && path.trim_matches('/').split('/').count() == 2 =>
            {
                comparisons::get(self, path_id(path))
            }
            ("POST", "/product/strategies/monte-carlo")
            | ("POST", "/product/strategies/research-sweeps/create") => {
                robustness::create(self, body)
            }
            ("POST", "/product/strategies/monte-carlo/status")
            | ("POST", "/product/strategies/monte-carlo/report")
            | ("POST", "/product/strategies/monte-carlo/replay")
            | ("POST", "/product/strategies/monte-carlo/export") => {
                robustness::get_from_body(self, &body)
            }
            ("POST", "/research/datasets")
            | ("POST", "/product/strategies/research/datasets/create") => {
                ServiceResponse::ok(api_result(research::create_dataset(self, body)))
            }
            ("POST", "/research/universes") => {
                ServiceResponse::ok(api_result(research::create_universe(self, body)))
            }
            ("GET", "/research/universes") => {
                ServiceResponse::ok(json!(research::list_universes(self)))
            }
            ("POST", "/research/jobs") | ("POST", "/product/strategies/research/jobs/create") => {
                ServiceResponse::ok(api_result(research::create_job(self, body)))
            }
            ("GET", "/research/jobs") => ServiceResponse::ok(json!(research::list_jobs(self))),
            ("POST", "/product/strategies/research/jobs/status") => {
                ServiceResponse::ok(api_result(research::job_status(self, body)))
            }
            ("GET", "/research/artifacts") => {
                ServiceResponse::ok(json!(backtest::list_artifacts(self)))
            }
            ("GET", "/research/sweeps") => ServiceResponse::ok(json!(research::list_sweeps(self))),
            ("POST", "/research/sweeps") => robustness::create(self, body),
            ("POST", "/product/strategies/research/promote-to-paper") => {
                ServiceResponse::ok(api_result(research::promote_to_paper(&body)))
            }
            ("POST", "/run") | ("POST", "/product/run-center/run-once") => {
                orders::run_once_response(self, body)
            }
            ("GET", "/runs") => ServiceResponse::ok(json!([self.run_once(json!({}))])),
            ("GET", "/orders") => ServiceResponse::ok(json!(orders::list_orders(self))),
            ("GET", "/orders/attempts") => ServiceResponse::ok(json!(orders::list_attempts(self))),
            ("POST", path) if path.starts_with("/orders/") && path.ends_with("/status-events") => {
                ServiceResponse::ok(orders::status_event(self, path, body))
            }
            ("POST", "/orders/status-stream") | ("POST", "/orders/status-stream/watch") => {
                ServiceResponse::ok(orders::status_stream(self))
            }
            ("POST", "/orders/reconcile") => ServiceResponse::ok(json!(orders::reconcile(self))),
            ("POST", "/orders/broker-recovery") => broker_recovery::recover(self, body),
            ("GET", "/exit-watches") => ServiceResponse::ok(json!([])),
            ("POST", "/exit-watches/process") => ServiceResponse::ok(
                json!({"processed": 0, "submit_orders": body.get("submit_orders").and_then(Value::as_bool).unwrap_or(false)}),
            ),
            ("GET", "/risk/status") => ServiceResponse::ok(risk::status(self)),
            ("POST", "/risk/reconcile") => ServiceResponse::ok(risk::reconcile(body)),
            ("GET", "/portfolio/positions") => ServiceResponse::ok(json!(positions::list(self))),
            ("POST", "/portfolio/positions/sync") => {
                ServiceResponse::ok(json!(positions::sync(self)))
            }
            ("PUT", "/risk/account") | ("POST", "/risk/account/sync") => {
                ServiceResponse::ok(risk::account_sync(body))
            }
            ("POST", "/risk/margin-preview") => ServiceResponse::ok(risk::margin_preview()),
            ("GET", "/risk/reservations") => ServiceResponse::ok(json!(risk::reservations(self))),
            ("POST", "/risk/stress-test") => ServiceResponse::ok(risk::stress_test(body)),
            ("POST", "/risk/portfolio-overlay")
            | ("POST", "/product/strategies/portfolio-risk")
            | ("POST", "/product/strategies/portfolio-risk/report")
            | ("POST", "/product/strategies/portfolio-risk/replay")
            | ("POST", "/product/strategies/portfolio-risk/export") => {
                let request = portfolio_risk_body_from(&body);
                let snapshot = self.portfolio_risk_overlay(request);
                ServiceResponse::ok(report_envelope::portfolio_risk_surface(
                    snapshot,
                    &strategy_id_from(&body),
                    "http://127.0.0.1:3001",
                    self.db(),
                ))
            }
            ("POST", "/execution/fill-quality")
            | ("POST", "/product/strategies/fill-quality")
            | ("POST", "/product/strategies/fill-quality/inspect")
            | ("POST", "/product/strategies/fill-quality/report")
            | ("POST", "/product/strategies/fill-quality/replay")
            | ("POST", "/product/strategies/fill-quality/export") => {
                ServiceResponse::ok(self.fill_quality_analysis(fill_quality_body_from(&body)))
            }
            ("POST", "/execution/lifecycle-calendar")
            | ("POST", "/product/strategies/lifecycle-calendar")
            | ("POST", "/product/strategies/lifecycle-calendar/list")
            | ("POST", "/product/strategies/lifecycle-calendar/inspect")
            | ("POST", "/product/strategies/lifecycle-calendar/export")
            | ("POST", "/product/strategies/lifecycle-calendar/replay") => {
                ServiceResponse::ok(self.lifecycle_calendar(lifecycle_calendar_body_from(&body)))
            }
            ("POST", "/journal/attribution-review")
            | ("POST", "/product/strategies/attribution-journal")
            | ("POST", "/product/strategies/attribution-journal/review")
            | ("POST", "/product/strategies/attribution-journal/report")
            | ("POST", "/product/strategies/attribution-journal/inspect")
            | ("POST", "/product/strategies/attribution-journal/replay")
            | ("POST", "/product/strategies/attribution-journal/export") => ServiceResponse::ok(
                self.attribution_journal_analysis(attribution_journal_body_from(&body)),
            ),
            ("POST", "/research/notebook")
            | ("POST", "/product/strategies/research-notebook")
            | ("POST", "/product/strategies/research-notebook/create")
            | ("POST", "/product/strategies/research-notebook/compose") => ServiceResponse::ok(
                self.research_notebook_operation("compose", research_notebook_body_from(&body)),
            ),
            ("POST", "/product/strategies/research-notebook/attach") => ServiceResponse::ok(
                self.research_notebook_operation("attach", research_notebook_body_from(&body)),
            ),
            ("POST", "/product/strategies/research-notebook/list") => ServiceResponse::ok(
                self.research_notebook_operation("list", research_notebook_body_from(&body)),
            ),
            ("POST", "/product/strategies/research-notebook/inspect") => ServiceResponse::ok(
                self.research_notebook_operation("inspect", research_notebook_body_from(&body)),
            ),
            ("POST", "/product/strategies/research-notebook/export") => ServiceResponse::ok(
                self.research_notebook_operation("export", research_notebook_body_from(&body)),
            ),
            ("POST", "/product/strategies/research-notebook/replay") => ServiceResponse::ok(
                self.research_notebook_operation("replay", research_notebook_body_from(&body)),
            ),
            ("GET", "/marketdata/bars") => ServiceResponse::ok(marketdata::bars(body)),
            ("POST", "/marketdata/conformance") => ServiceResponse::ok(
                crate::conformance::marketdata_conformance(&["local-data"], true),
            ),
            ("GET", "/marketdata/quote") => ServiceResponse::ok(marketdata::quote(body)),
            ("POST", "/marketdata/indicators") => ServiceResponse::ok(marketdata::indicators(body)),
            ("GET", "/marketdata/options/chain") => ServiceResponse::ok(options::chain(body)),
            ("POST", "/marketdata/options/select") => ServiceResponse::ok(options::select(body)),
            ("POST", "/marketdata/instruments/resolve")
            | ("POST", "/instruments/resolve")
            | ("POST", "/instruments/normalize") => {
                ServiceResponse::ok(self.instrument_context(&strategy_id_from(&body)))
            }
            ("GET", "/marketdata/instruments/aliases") | ("GET", "/instruments/aliases") => {
                ServiceResponse::ok(
                    json!({"aliases": ["BTC/USD", "BTCUSD"], "canonicalId": "ul:crypto:btc-usd"}),
                )
            }
            ("POST", "/marketdata/instrument-packs/validate")
            | ("POST", "/instruments/packs/validate") => {
                ServiceResponse::ok(marketdata::validate_pack(body))
            }
            ("POST", "/marketdata/instrument-packs/install")
            | ("POST", "/instruments/packs/install") => {
                ServiceResponse::ok(marketdata::install_pack(self, body))
            }
            ("GET", "/marketdata/instrument-packs") | ("GET", "/instruments/packs") => {
                ServiceResponse::ok(marketdata::list_packs(self))
            }
            ("GET", "/scheduler/status") => ServiceResponse::ok(scheduler::status(self)),
            ("POST", "/scheduler/start") | ("POST", "/product/scheduler/start") => {
                ServiceResponse::ok(scheduler::start(self, body))
            }
            ("POST", "/scheduler/stop") | ("POST", "/product/scheduler/stop") => {
                ServiceResponse::ok(scheduler::stop(self))
            }
            ("POST", "/scheduler/tick") | ("POST", "/scheduler/run") => {
                ServiceResponse::ok(workers::run(self, body))
            }
            ("GET", "/journal/events") => match journal_view::events(self) {
                Ok(events) => ServiceResponse::ok(json!(events)),
                Err(response) => response,
            },
            ("GET", "/journal/export") => journal_view::export(self),
            ("POST", "/journal/replay") => replay::journal_replay(self),
            ("POST", "/journal/replay-report") => replay::journal_replay(self),
            ("POST", "/journal/replay-harness") => replay::journal_replay_harness(self, body),
            ("POST", "/product/viewer") => ServiceResponse::ok(self.workspace()),
            ("POST", "/product/strategies/get") | ("POST", "/product/strategies/home") => {
                ServiceResponse::ok(
                    json!({"strategy": self.strategy_value(&strategy_id_from(&body)), "evidence": workspace_evidence(self)}),
                )
            }
            ("POST", "/product/strategies/version-history") => ServiceResponse::ok(json!({
                "strategy": self.strategy_value(&strategy_id_from(&body)),
                "versions": self.strategy_versions(&strategy_id_from(&body)),
            })),
            ("POST", "/product/strategies/instrument-context") => {
                ServiceResponse::ok(self.instrument_context(&strategy_id_from(&body)))
            }
            ("POST", "/product/strategies/validate-expression") => ServiceResponse::ok(api_result(
                json!({"valid": true, "expression": body.get("expression").cloned().unwrap_or_default()}),
            )),
            ("POST", "/product/strategies/ai-draft") => ServiceResponse::ok(api_result(
                ai::strategy_draft_payload(
                    &ai::load_ai_gateway_config(),
                    body["brief"].as_str().unwrap_or(""),
                    body["market"].as_str().unwrap_or("crypto"),
                    body["horizon"].as_str().unwrap_or("intraday"),
                )
                .unwrap_or_else(|error| json!({"error": error})),
            )),
            ("POST", "/product/strategies/publish") => {
                ServiceResponse::ok(self.publish_strategy(body))
            }
            ("POST", "/product/strategies/duplicate") => {
                ServiceResponse::ok(self.duplicate_strategy(body))
            }
            ("POST", "/product/strategies/archive") => ServiceResponse::ok(api_result(
                json!({"strategy": self.set_strategy_status(&strategy_id_from(&body), "paused")}),
            )),
            ("POST", "/product/strategies/restore") => ServiceResponse::ok(api_result(
                json!({"strategy": self.set_strategy_status(&strategy_id_from(&body), "draft")}),
            )),
            ("POST", "/product/strategies/builder-state") => {
                ServiceResponse::ok(self.builder_state(&strategy_id_from(&body)))
            }
            ("POST", "/product/strategies/validate-draft") => {
                ServiceResponse::ok(self.validate_builder_draft(body))
            }
            ("POST", "/product/strategy-execution-configs/save") => {
                ServiceResponse::ok(api_result(execution::save_config(self, body)))
            }
            ("POST", "/product/strategy-execution-configs/activation-readiness") => {
                ServiceResponse::ok(api_result(execution::activation_readiness(self, body)))
            }
            ("POST", "/product/strategy-execution-activations/activate") => {
                let activation = execution::activate(self, body);
                if activation["status"].as_str() == Some("blocked") {
                    ServiceResponse::bad_request_with_details(
                        "activation_capability_blocked",
                        activation,
                    )
                } else if activation["status"].as_str() == Some("failed") {
                    let code = activation["error"]["code"]
                        .as_str()
                        .unwrap_or("activation_dependency_unavailable")
                        .to_string();
                    ServiceResponse::error_with_details(503, &code, true, activation)
                } else {
                    ServiceResponse::ok(api_result(activation))
                }
            }
            ("POST", "/product/strategy-execution-activations/deactivate") => {
                ServiceResponse::ok(api_result(execution::deactivate(self, body)))
            }
            ("POST", "/product/strategy-execution-activations/control") => {
                ServiceResponse::ok(api_result(execution::control(self, body)))
            }
            ("POST", path)
                if path.starts_with("/product/strategy-execution-activations/")
                    && path.ends_with("/ticks/evaluate") =>
            {
                execution::evaluate_tick_response(self, path, body)
            }
            ("POST", "/product/strategies/execution-workspace") => {
                ServiceResponse::ok(execution::workspace(self, &strategy_id_from(&body), &body))
            }
            ("POST", "/product/run-center") => ServiceResponse::ok(execution::run_center(self)),
            ("POST", "/product/connect-workspace") => {
                ServiceResponse::ok(providers::connect_workspace(self))
            }
            ("POST", "/product/providers/credentials/test") => ServiceResponse::ok(api_result(
                credentials::test(self, &provider_ref_from(&body), body),
            )),
            ("POST", "/product/strategies/research-workspace") => {
                ServiceResponse::ok(self.research_workspace(&strategy_id_from(&body)))
            }
            ("POST", "/product/research/rollup") => ServiceResponse::ok(research::rollup(self)),
            ("POST", "/product/alerts/center") => ServiceResponse::ok(json!({
                "alerts": [{
                    "id": "local_test_alert",
                    "severity": "info",
                    "status": "open",
                    "title": "Local repair detail",
                    "action": "review",
                    "repairLink": "/app/run",
                    "detail": {"strategyId": "strat_local_btc_demo"},
                    "evidence": {"eventType": "local.alert"},
                }],
                "counts": {"open": 1},
            })),
            ("POST", "/product/alerts/acknowledge") => ServiceResponse::ok(api_result(
                json!({"id": body.get("id").or_else(|| body.get("alertId")).cloned().unwrap_or(json!("local_test_alert")), "acknowledged": true}),
            )),
            ("POST", "/product/discover-workspace") => ServiceResponse::ok(
                json!({"starters": demos::demo_descriptors().into_iter().map(|item| json!({"id": item.id, "title": item.title})).collect::<Vec<_>>(), "proofPackages": []}),
            ),
            ("POST", "/product/strategies/share-workspace") => ServiceResponse::ok(
                json!({"strategy": self.strategy_value(&strategy_id_from(&body)), "snapshots": [], "evidence": workspace_evidence(self)}),
            ),
            ("POST", "/product/strategies/share-snapshots/create") => {
                ServiceResponse::error(501, "durable_share_snapshots_unavailable", false)
            }
            ("POST", "/product/strategies/share-snapshots/revoke") => {
                ServiceResponse::error(404, "snapshot_unavailable", false)
            }
            ("POST", "/product/share-snapshots/view")
            | ("POST", "/product/share-snapshots/export") => {
                ServiceResponse::error(404, "snapshot_unavailable", false)
            }
            ("POST", "/product/reports/envelope") => {
                let kind = body
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("backtest");
                if kind == "scenario_valuation" {
                    ServiceResponse::bad_request(
                        "scenario_valuation_requires_durable_derivatives_analysis",
                    )
                } else if kind == "monte_carlo" {
                    ServiceResponse::bad_request("monte_carlo_requires_durable_robustness_run")
                } else if kind == "portfolio_risk" {
                    let request = portfolio_risk_body_from(&body);
                    let snapshot = self.portfolio_risk_overlay(request);
                    ServiceResponse::ok(report_envelope::portfolio_risk_surface(
                        snapshot,
                        &strategy_id_from(&body),
                        "http://127.0.0.1:3001",
                        self.db(),
                    ))
                } else if kind == "fill_quality" {
                    ServiceResponse::ok(self.fill_quality_analysis(fill_quality_body_from(&body)))
                } else if kind == "lifecycle_calendar" {
                    ServiceResponse::ok(
                        self.lifecycle_calendar(lifecycle_calendar_body_from(&body)),
                    )
                } else {
                    ServiceResponse::ok(report_envelope::report_envelope(
                        kind,
                        &strategy_id_from(&body),
                        body.get("backtestId")
                            .or_else(|| body.get("backtest_id"))
                            .and_then(Value::as_str)
                            .unwrap_or("backtest-local"),
                        "http://127.0.0.1:3001",
                    ))
                }
            }
            ("POST", "/product/strategies/scenario-valuation") => derivatives::create(self, body),
            ("POST", "/marketdata/selectors/evaluate")
            | ("POST", "/marketdata/selectors/explain") => ServiceResponse::ok(
                json!({"run_id": "selector_run_demo", "accepted_candidates": [], "rejected_candidates": [], "warnings": []}),
            ),
            ("POST", path) if path.starts_with("/execution/position-lifecycle/") => {
                ServiceResponse::ok(
                    json!({"status": "ok", "path": path, "timeline": [], "warnings": []}),
                )
            }
            ("GET", "/status") | ("GET", "/admin/status") => {
                ServiceResponse::ok(json!({"runtime": "rust", "db": self.db, "healthy": true}))
            }
            ("GET", "/admin/control-plane") => {
                ServiceResponse::ok(json!({"commands": self.control_plane_commands()}))
            }
            ("GET", "/admin/finance-authority") => ServiceResponse::ok(
                self.runtime
                    .finance_authority
                    .audit(self.runtime.storage.as_ref()),
            ),
            ("GET", "/admin/workspace") => ServiceResponse::ok(self.workspace()),
            ("GET", "/admin/providers") => {
                ServiceResponse::ok(json!({"providers": self.providers()}))
            }
            ("GET", "/admin/scheduler") => ServiceResponse::ok(workers::status(self)),
            ("GET", "/admin/risk") => ServiceResponse::ok(risk::status(self)),
            _ => ServiceResponse::not_found(&normalized_path),
        }
    }

    pub fn execute_graphql(&self, request: Value) -> Value {
        self.execute_graphql_with_session(request, None)
    }

    fn execute_graphql_with_session(
        &self,
        request: Value,
        authenticated_studio: Option<&auth::SessionValidationResult>,
    ) -> Value {
        let op = graphql_operation_name(&request);
        let request = self.enrich_graphql_request(&request, &op);
        if let Some(path) = graphql_object_authorization_route(&op) {
            let variables = request
                .get("variables")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let authorization_body = variables.get("request").cloned().unwrap_or(variables);
            if let Some(response) = object_authorization::authorize_private_request(
                self,
                "POST",
                path,
                &authorization_body,
            ) {
                return response.body;
            }
        }
        let command_envelope = match control_plane::graphql_envelope(&request)
            .map(|envelope| inherit_trusted_graphql_correlation(envelope, &request))
        {
            Ok(envelope) => match control_plane::prepare_command(
                self.runtime.storage.as_ref(),
                self.runtime.finance_authority.as_ref(),
                envelope.clone(),
            ) {
                Ok(record) if record.duplicate && envelope.side_effect_class.as_str() == "read" => {
                    // Reads retain a stable audit identity, but their response must reflect
                    // current durable state rather than replaying a previous snapshot.
                    Some(envelope)
                }
                Ok(record) if record.duplicate => {
                    return record
                        .response_status
                        .map(|status| {
                            record.response_body.clone().map_or_else(
                                || graphql_duplicate_response(status),
                                |body| graphql_duplicate_response_with_body(status, body),
                            )
                        })
                        .unwrap_or_else(
                            || json!({"errors": [{"message": "idempotency_in_progress"}]}),
                        );
                }
                Ok(_) => Some(envelope),
                Err(error) if error.starts_with("finance_authority:") => {
                    if op == "ActivateExecution" {
                        return json!({
                            "errors": [{
                                "message": "finance_authority_denied",
                                "extensions": {
                                    "details": execution::activation_blocked_preflight(
                                        self,
                                        request["variables"].clone()
                                    )
                                }
                            }]
                        });
                    }
                    return json!({"errors": [{"message": "finance_authority_denied"}]});
                }
                Err(_) => {
                    return json!({"errors": [{"message": "control_plane_persistence_failed"}]});
                }
            },
            Err(_) => None,
        };
        if op == "__schema" || request["query"].as_str().unwrap_or("").contains("__schema") {
            let response = json!({"data": {"schema": graphql_schema_text()}});
            return self.complete_graphql_command_or_error(&command_envelope, 200, response);
        }
        if op.is_empty() {
            let response =
                json!({"errors": [{"message": "Unknown TradeAssembly GraphQL operation: "} ]});
            return self.complete_graphql_command_or_error(&command_envelope, 400, response);
        }
        if !GRAPHQL_OPERATIONS.contains(&op.as_str()) {
            let response = json!({"errors": [{"message": format!("Unknown TradeAssembly GraphQL operation: {op}")}]});
            return self.complete_graphql_command_or_error(&command_envelope, 400, response);
        }
        let mut variables = request
            .get("variables")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let Some(envelope) = &command_envelope {
            variables = with_control_plane_metadata(variables, envelope);
        }
        let strategy_id = strategy_id_from(&variables);
        let field = graphql_field(&op);
        let value = match op.as_str() {
            "LocalWorkspace" => self.workspace(),
            "StrategyLibrary" => {
                json!({"strategies": self.strategies(), "lifecycle": strategy_lifecycle(), "evidence": workspace_evidence(self)})
            }
            "StrategyHome" | "StrategyDetail" => {
                json!({"strategy": self.strategy_value(&strategy_id), "evidence": workspace_evidence(self), "lifecycle": strategy_lifecycle()})
            }
            "StrategyInstrumentContext" => self.instrument_context(&strategy_id),
            "StrategyVersionHistory" => {
                json!({"strategy": self.strategy_value(&strategy_id), "versions": [{"id": "ver_local", "version": 1, "spec": spec::btc_exit_demo_spec_payload()}]})
            }
            "StrategyBuilder" => self.builder_state(&strategy_id),
            "StrategyContractMetadata" => json!({
                "stages": self.dispatch_http("GET", "/strategy/contracts/stages", json!({})).body,
                "substeps": self.dispatch_http("GET", "/strategy/contracts/substeps", json!({})).body,
                "indicatorSchema": self.dispatch_http("GET", "/strategy/contracts/indicator-schema", json!({})).body,
                "providerCompatibility": self.dispatch_http("POST", "/strategy/contracts/compatibility:resolve", variables.clone()).body,
            }),
            "StrategyResearch" | "StrategyResearchWorkspace" => {
                self.research_workspace(&strategy_id)
            }
            "StrategyExecution" | "StrategyExecutionWorkspace" => {
                self.execution_workspace(&strategy_id, &variables)
            }
            "RunCenter" => {
                self.dispatch_http("POST", "/product/run-center", json!({}))
                    .body
            }
            "ResearchRollup" => {
                self.dispatch_http("POST", "/product/research/rollup", json!({}))
                    .body
            }
            "DatasetIngestionList" => {
                self.dispatch_http("GET", "/dataset-ingestions", variables)
                    .body
            }
            "DatasetIngestionGet" | "DatasetIngestionStatus" => {
                self.dispatch_http(
                    "GET",
                    &format!(
                        "/dataset-ingestions/{}",
                        variables["ingestionId"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "AlertCenter" => {
                self.dispatch_http("POST", "/product/alerts/center", json!({}))
                    .body
            }
            "DiscoverWorkspace" => {
                self.dispatch_http("POST", "/product/discover-workspace", json!({}))
                    .body
            }
            "StrategyShareWorkspace" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/share-workspace",
                    variables.clone(),
                )
                .body
            }
            "ConnectWorkspace" | "Providers" => {
                self.dispatch_http("POST", "/product/connect-workspace", json!({}))
                    .body
            }
            "PluginsProviders" => self.plugins_providers(),
            "PluginManifests" => {
                self.dispatch_http("GET", "/plugins/manifests", json!({}))
                    .body
            }
            "PluginInstances" => {
                self.dispatch_http("GET", "/plugins/instances", json!({}))
                    .body
            }
            "PluginInstance" => {
                self.dispatch_http(
                    "GET",
                    &format!(
                        "/plugins/instances/{}",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    json!({}),
                )
                .body
            }
            "Entitlements" => self.dispatch_http("GET", "/entitlements", json!({})).body,
            "ResolveCapability" => {
                self.dispatch_http(
                    "POST",
                    "/strategy/contracts/capability:resolve",
                    variables.clone(),
                )
                .body
            }
            "ResolveCapabilityGraph" => {
                self.dispatch_http("POST", "/capabilities/graph/resolve", variables.clone())
                    .body
            }
            "SaveCapabilityGraphRevision" => {
                self.dispatch_http("POST", "/capability-graph-revisions", variables.clone())
                    .body
            }
            "CapabilityGraphRevision" => {
                self.dispatch_http(
                    "GET",
                    &format!(
                        "/capability-graph-revisions/{}",
                        variables["revisionId"].as_str().unwrap_or_default()
                    ),
                    json!({}),
                )
                .body
            }
            "CheckCapabilityGraphRevision" => {
                self.dispatch_http(
                    "POST",
                    "/capability-graph-revisions/currentness",
                    variables.clone(),
                )
                .body
            }
            "AdminPluginManifestRegistry" => self.plugins_providers(),
            "StoreCredentials" => {
                self.dispatch_http(
                    "POST",
                    &format!("/providers/{}/credentials", provider_ref_from(&variables)),
                    variables,
                )
                .body
            }
            "TestCredentials" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/providers/{}/credentials/test",
                        provider_ref_from(&variables)
                    ),
                    variables,
                )
                .body
            }
            "InstallPluginManifest" => {
                self.dispatch_http("POST", "/plugins/manifests", variables)
                    .body
            }
            "InstallPluginPackage" => {
                self.dispatch_http("POST", "/plugins/packages", variables)
                    .body
            }
            "CreatePluginInstance" => {
                self.dispatch_http("POST", "/plugins/instances", variables)
                    .body
            }
            "ConfigurePluginInstance" => {
                self.dispatch_http(
                    "PUT",
                    &format!(
                        "/plugins/instances/{}/configuration",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "EnablePluginInstance" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/plugins/instances/{}:enable",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "DisablePluginInstance" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/plugins/instances/{}:disable",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "UpgradePluginInstance" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/plugins/instances/{}:upgrade",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "RollbackPluginInstance" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/plugins/instances/{}:rollback",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "RemovePluginInstance" => {
                self.dispatch_http(
                    "DELETE",
                    &format!(
                        "/plugins/instances/{}",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "StartPluginOAuth" => {
                self.dispatch_http("POST", "/plugins/oauth/start", variables)
                    .body
            }
            "PluginOAuthStatus" => {
                self.dispatch_http("POST", "/plugins/oauth/status", variables)
                    .body
            }
            "DisconnectPluginOAuth" => {
                self.dispatch_http("POST", "/plugins/oauth/disconnect", variables)
                    .body
            }
            "StorePluginCredentials" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/plugins/instances/{}/credentials",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "RevokePluginCredentials" => {
                self.dispatch_http(
                    "DELETE",
                    &format!(
                        "/plugins/instances/{}/credentials",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "RefreshPluginHealth" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/plugins/instances/{}/health:refresh",
                        variables["instanceRef"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "RevokeEntitlementOverride" => {
                self.dispatch_http(
                    "DELETE",
                    &format!(
                        "/entitlements/{}",
                        variables["grantId"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "CreateStrategy" => {
                self.dispatch_http("POST", "/product/strategies/create", variables)
                    .body
            }
            "CreateStrategyAiDraft" => api_result(
                ai::strategy_draft_payload(
                    &ai::load_ai_gateway_config(),
                    variables["brief"].as_str().unwrap_or(""),
                    variables["market"].as_str().unwrap_or("crypto"),
                    variables["horizon"].as_str().unwrap_or("intraday"),
                )
                .unwrap_or_else(|error| json!({"error": error})),
            ),
            "SaveBuilderDraft" => {
                self.dispatch_http("POST", "/product/strategies/save-draft", variables)
                    .body
            }
            "SelectStrategySemanticNode" => {
                self.dispatch_http("POST", "/product/strategies/semantic-selection", variables)
                    .body
            }
            "ProposeStrategyPatch" => {
                self.dispatch_http("POST", "/product/strategies/proposals/create", variables)
                    .body
            }
            "ReviewStrategyPatch" | "ApplyStrategyPatch" => {
                self.dispatch_http("POST", "/product/strategies/proposals/review", variables)
                    .body
            }
            "SightlineSessionState" => {
                self.dispatch_http("GET", "/product/sightline/session-state", variables)
                    .body
            }
            "PublishSightlineStrategyEditorSurface" => {
                self.dispatch_http(
                    "POST",
                    "/product/sightline/strategy-editor/publish",
                    variables,
                )
                .body
            }
            "PublishSightlineStudioSurface" => {
                self.dispatch_http(
                    "POST",
                    "/product/sightline/studio-surface/publish",
                    variables,
                )
                .body
            }
            "SetSightlineSelection" => {
                if let Some(session) = authenticated_studio {
                    self.set_sightline_selection_for_authenticated_user(session, variables)
                } else {
                    self.dispatch_http("POST", "/product/sightline/selections/set", variables)
                        .body
                }
            }
            "ObserveStudioSetup" => {
                match authenticated_studio.filter(|session| session.authenticated) {
                    Some(session) => match session.principal.as_ref() {
                        Some(principal) => {
                            broker_onboarding::observe_studio_setup(self, variables, principal)
                        }
                        None => ServiceResponse::unauthorized("studio_identity_required").body,
                    },
                    None => ServiceResponse::unauthorized("studio_authentication_required").body,
                }
            }
            "ShareSightlineSelection" => {
                if let Some(session) = authenticated_studio {
                    self.share_sightline_selection_for_authenticated_user(session, variables)
                } else {
                    self.dispatch_http("POST", "/product/sightline/selections/share", variables)
                        .body
                }
            }
            "SightlineGetContext" => {
                self.dispatch_http("POST", "/product/sightline/context/get", variables)
                    .body
            }
            "ConnectSightlineAgent" => {
                self.dispatch_http("POST", "/product/sightline/agents/connect", variables)
                    .body
            }
            "SightlineUpdatePresence" => {
                self.dispatch_http("POST", "/product/sightline/presence/update", variables)
                    .body
            }
            "SightlineCreateProposal" => {
                self.dispatch_http("POST", "/product/sightline/proposals/create", variables)
                    .body
            }
            "SightlineRequestApproval" => {
                self.dispatch_http("POST", "/product/sightline/approvals/request", variables)
                    .body
            }
            "SightlineNavigateSurface" => {
                self.dispatch_http("POST", "/product/sightline/navigation/request", variables)
                    .body
            }
            "SightlineFocusNode" => {
                self.dispatch_http("POST", "/product/sightline/nodes/focus", variables)
                    .body
            }
            "SightlineHighlightNode" => {
                self.dispatch_http("POST", "/product/sightline/nodes/highlight", variables)
                    .body
            }
            "SightlineEvents" => {
                self.dispatch_http("GET", "/product/sightline/events", variables)
                    .body
            }
            "ValidateBuilderDraft" => {
                self.dispatch_http("POST", "/product/strategies/validate-draft", variables)
                    .body
            }
            "ValidateStrategyExpression" => {
                self.dispatch_http("POST", "/product/strategies/validate-expression", variables)
                    .body
            }
            "PublishStrategy" => {
                self.dispatch_http("POST", "/product/strategies/publish", variables)
                    .body
            }
            "DuplicateStrategy" => {
                self.dispatch_http("POST", "/product/strategies/duplicate", variables)
                    .body
            }
            "ArchiveStrategy" => {
                self.dispatch_http("POST", "/product/strategies/archive", variables)
                    .body
            }
            "RestoreStrategy" => {
                self.dispatch_http("POST", "/product/strategies/restore", variables)
                    .body
            }
            "CompileStrategyPreview" => {
                self.dispatch_http(
                    "POST",
                    "/strategy/contracts/envelope-preview",
                    variables
                        .get("spec")
                        .cloned()
                        .unwrap_or_else(spec::btc_exit_demo_spec_payload),
                )
                .body
            }
            "RunStrategyResearch" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research-runs/create",
                    nested_graphql_request(&variables),
                )
                .body
            }
            "RunBacktest" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/backtests/run",
                    nested_graphql_request(&variables),
                )
                .body
            }
            "BacktestRuns" => self.dispatch_http("GET", "/backtests", variables).body,
            "BacktestRun" => {
                backtest_lifecycle::get(self, variables["runId"].as_str().unwrap_or_default()).body
            }
            "BacktestReport" => {
                backtest_lifecycle::report(self, variables["runId"].as_str().unwrap_or_default())
                    .body
            }
            "CancelBacktest" => {
                let run_id = variables["runId"].as_str().unwrap_or_default().to_string();
                backtest_lifecycle::cancel(self, &run_id, variables).body
            }
            "RetryBacktest" => {
                let run_id = variables["runId"].as_str().unwrap_or_default().to_string();
                backtest_lifecycle::retry(self, &run_id, variables).body
            }
            "ProcessBacktest" => {
                let run_id = variables["runId"].as_str().unwrap_or_default().to_string();
                backtest_lifecycle::process_run(
                    self,
                    variables["worker"]
                        .as_str()
                        .unwrap_or("local-backtest-worker"),
                    &run_id,
                )
                .body
            }
            "CreateRobustnessRun" => {
                self.dispatch_http(
                    "POST",
                    "/robustness-runs",
                    nested_graphql_request(&variables),
                )
                .body
            }
            "RobustnessRuns" => {
                self.dispatch_http("GET", "/robustness-runs", variables)
                    .body
            }
            "RobustnessRun" => {
                robustness::get(self, variables["runId"].as_str().unwrap_or_default()).body
            }
            "RobustnessReport" => {
                robustness::report(self, variables["runId"].as_str().unwrap_or_default()).body
            }
            "ProcessRobustnessRun" => {
                let run_id = variables["runId"].as_str().unwrap_or_default().to_string();
                if let Err(response) = self.require_object("robustness_run", &run_id) {
                    response.body
                } else {
                    let mut worker_service = self.clone();
                    worker_service.invocation_principal = None;
                    let processed =
                        worker_service.dispatch_http("POST", "/robustness-runs:process", variables);
                    if processed.body["runId"].as_str() == Some(run_id.as_str()) {
                        processed.body
                    } else {
                        robustness::get(self, &run_id).body
                    }
                }
            }
            "CancelRobustnessRun" => {
                let run_id = variables["runId"].as_str().unwrap_or_default().to_string();
                robustness::cancel(self, &run_id, variables).body
            }
            "RetryRobustnessRun" => {
                let run_id = variables["runId"].as_str().unwrap_or_default().to_string();
                robustness::retry(self, &run_id, variables).body
            }
            "ReplayRobustnessRun" => {
                robustness::replay(self, variables["runId"].as_str().unwrap_or_default()).body
            }
            "ExportRobustnessRun" => {
                robustness::export(
                    self,
                    variables["runId"].as_str().unwrap_or_default(),
                    variables["format"].as_str().unwrap_or("json"),
                )
                .body
            }
            "CreateDerivativesAnalysis" => {
                self.dispatch_http(
                    "POST",
                    "/derivatives-analyses",
                    nested_graphql_request(&variables),
                )
                .body
            }
            "DerivativesAnalyses" => {
                self.dispatch_http("GET", "/derivatives-analyses", variables)
                    .body
            }
            "DerivativesAnalysis" => {
                derivatives::get(self, variables["analysisId"].as_str().unwrap_or_default()).body
            }
            "DerivativesAnalysisExport" => {
                derivatives::export(
                    self,
                    variables["analysisId"].as_str().unwrap_or_default(),
                    variables["format"].as_str().unwrap_or("json"),
                )
                .body
            }
            "CreateResearchComparison" => {
                self.dispatch_http(
                    "POST",
                    "/research-comparisons",
                    nested_graphql_request(&variables),
                )
                .body
            }
            "ResearchComparisons" => {
                self.dispatch_http("GET", "/research-comparisons", variables)
                    .body
            }
            "ResearchComparison" => {
                comparisons::get(self, variables["comparisonId"].as_str().unwrap_or_default()).body
            }
            "ResearchComparisonExport" => {
                comparisons::export(
                    self,
                    variables["comparisonId"].as_str().unwrap_or_default(),
                    variables["format"].as_str().unwrap_or("json"),
                )
                .body
            }
            "ReplayBacktest" => {
                backtest_lifecycle::replay(self, variables["runId"].as_str().unwrap_or_default())
                    .body
            }
            "CreateDatasetIngestion" => {
                self.dispatch_http(
                    "POST",
                    "/dataset-ingestions",
                    nested_graphql_request(&variables),
                )
                .body
            }
            "CancelDatasetIngestion" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/dataset-ingestions/{}/cancel",
                        variables["ingestionId"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "VerifyDatasetIngestion" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/dataset-ingestions/{}/verify",
                        variables["ingestionId"].as_str().unwrap_or_default()
                    ),
                    variables,
                )
                .body
            }
            "BacktestReportExport" => {
                self.dispatch_http("POST", "/product/strategies/backtests/export", variables)
                    .body
            }
            "RunScenarioValuation" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/scenario-valuation",
                    nested_graphql_request(&variables),
                )
                .body
            }
            "RunMonteCarlo" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/monte-carlo",
                    nested_graphql_request(&variables),
                )
                .body
            }
            "RunPortfolioRisk" => {
                self.dispatch_http("POST", "/product/strategies/portfolio-risk", variables)
                    .body
            }
            "PortfolioRiskReport" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/portfolio-risk/report",
                    variables,
                )
                .body
            }
            "FillQualityInspect" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/fill-quality/inspect",
                    variables,
                )
                .body
            }
            "FillQualityReport" => {
                self.dispatch_http("POST", "/product/strategies/fill-quality/report", variables)
                    .body
            }
            "FillQualityReplay" => {
                self.dispatch_http("POST", "/product/strategies/fill-quality/replay", variables)
                    .body
            }
            "FillQualityExport" => {
                self.dispatch_http("POST", "/product/strategies/fill-quality/export", variables)
                    .body
            }
            "AttributionJournalReview" | "AttributionJournalReport" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/attribution-journal/report",
                    variables,
                )
                .body
            }
            "AttributionJournalInspect" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/attribution-journal/inspect",
                    variables,
                )
                .body
            }
            "AttributionJournalReplay" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/attribution-journal/replay",
                    variables,
                )
                .body
            }
            "AttributionJournalExport" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/attribution-journal/export",
                    variables,
                )
                .body
            }
            "LifecycleCalendarList" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/lifecycle-calendar/list",
                    variables,
                )
                .body
            }
            "LifecycleCalendarInspect" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/lifecycle-calendar/inspect",
                    variables,
                )
                .body
            }
            "LifecycleCalendarExport" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/lifecycle-calendar/export",
                    variables,
                )
                .body
            }
            "LifecycleCalendarReplay" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/lifecycle-calendar/replay",
                    variables,
                )
                .body
            }
            "ResearchNotebookCreate" | "ResearchNotebookCompose" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research-notebook/compose",
                    variables,
                )
                .body
            }
            "ResearchNotebookAttach" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research-notebook/attach",
                    variables,
                )
                .body
            }
            "ResearchNotebookList" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research-notebook/list",
                    variables,
                )
                .body
            }
            "ResearchNotebookInspect" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research-notebook/inspect",
                    variables,
                )
                .body
            }
            "ResearchNotebookExport" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research-notebook/export",
                    variables,
                )
                .body
            }
            "ResearchNotebookReplay" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research-notebook/replay",
                    variables,
                )
                .body
            }
            "MonteCarloStatus" => {
                robustness::get(self, variables["runId"].as_str().unwrap_or_default()).body
            }
            "MonteCarloReport" => {
                robustness::report(self, variables["runId"].as_str().unwrap_or_default()).body
            }
            "CreateResearchDataset" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research/datasets/create",
                    variables,
                )
                .body
            }
            "QueueStrategyResearchJob" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research/jobs/create",
                    variables,
                )
                .body
            }
            "StrategyResearchJobStatus" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research/jobs/status",
                    variables,
                )
                .body
            }
            "PromoteStrategyResearchToPaper" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research/promote-to-paper",
                    variables,
                )
                .body
            }
            "RunResearchSweep" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/research-sweeps/create",
                    variables,
                )
                .body
            }
            "CreateResearchUniverse" => {
                self.dispatch_http("POST", "/research/universes", variables)
                    .body
            }
            "SaveExecutionConfig" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-configs/save",
                    variables,
                )
                .body
            }
            "ActivationReadiness" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-configs/activation-readiness",
                    variables,
                )
                .body
            }
            "ActivateExecution" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-activations/activate",
                    variables,
                )
                .body
            }
            "ControlStrategyExecution" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-activations/control",
                    variables,
                )
                .body
            }
            "DeactivateStrategyExecution" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-activations/deactivate",
                    variables,
                )
                .body
            }
            "UpdateProviderInstance" => {
                self.dispatch_http("POST", "/product/providers/update-instance", variables)
                    .body
            }
            "EnableProviderInstance" => {
                self.dispatch_http("POST", "/product/providers/enable-instance", variables)
                    .body
            }
            "DisableProviderInstance" => {
                self.dispatch_http("POST", "/product/providers/disable-instance", variables)
                    .body
            }
            "AcknowledgeAlert" => {
                self.dispatch_http("POST", "/product/alerts/acknowledge", variables)
                    .body
            }
            "CreateStrategyShareSnapshot" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/share-snapshots/create",
                    variables,
                )
                .body
            }
            "RevokeStrategyShareSnapshot" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategies/share-snapshots/revoke",
                    variables,
                )
                .body
            }
            "RunStrategyOnce" => {
                self.dispatch_http("POST", "/product/run-center/run-once", variables)
                    .body
            }
            "SchedulerStart" => {
                self.dispatch_http("POST", "/product/scheduler/start", variables)
                    .body
            }
            "SchedulerRun" => self.dispatch_http("POST", "/scheduler/run", variables).body,
            "SchedulerStop" => {
                self.dispatch_http("POST", "/product/scheduler/stop", variables)
                    .body
            }
            "ReconcileOrders" => api_result(
                self.dispatch_http("POST", "/orders/reconcile", variables)
                    .body,
            ),
            _ => json!({}),
        };
        let status = graphql_value_status(&value);
        let response = if status >= 400 {
            json!({"errors": [{"message": graphql_error_code(&value)}]})
        } else {
            json!({"data": {field: value}})
        };
        self.complete_graphql_command_or_error(&command_envelope, status, response)
    }

    pub fn portfolio_risk_overlay(&self, request: Value) -> Value {
        risk::portfolio_overlay(self, request)
    }

    pub fn latest_portfolio_risk_overlay(&self) -> Option<Value> {
        risk::latest_portfolio_overlay(self)
    }

    pub fn fill_quality_analysis(&self, request: Value) -> Value {
        fill_quality::analyze(self, request)
    }

    pub fn lifecycle_calendar(&self, request: Value) -> Value {
        lifecycle_calendar::build_timeline(self, request)
    }

    pub fn latest_lifecycle_calendar(&self) -> Option<Value> {
        lifecycle_calendar::latest_timeline(self)
    }

    pub fn research_notebook(&self, request: Value) -> Value {
        self.research_notebook_operation("compose", request)
    }

    pub fn research_notebook_operation(&self, operation: &str, request: Value) -> Value {
        research_notebook::operation(self, operation, request)
    }

    pub fn latest_research_notebook(&self) -> Option<Value> {
        research_notebook::latest(self)
    }

    pub fn attribution_journal_analysis(&self, request: Value) -> Value {
        attribution_journal::analyze(self, request)
    }

    pub fn latest_attribution_journal(&self) -> Option<Value> {
        attribution_journal::latest(self)
    }

    /// Read setup evidence without creating deployments or resuming schedulers.
    pub fn onboarding_snapshot(&self) -> Value {
        onboarding::snapshot(self)
    }

    pub fn call_mcp_tool(&self, name: &str, arguments: Value) -> Value {
        if matches!(
            name,
            "tradeassembly.account.login"
                | "tradeassembly.onboarding.start"
                | "tradeassembly.onboarding.status"
                | "tradeassembly.onboarding.cancel"
                | "tradeassembly.account.login.status"
                | "tradeassembly.account.status"
                | "tradeassembly.setup.inspect"
        ) {
            return mcp::tool_error(
                name,
                "identity_transport_required",
                "Use the local MCP connection for identity and setup inspection.",
                None,
            );
        }
        let broker_onboarding_call = matches!(
            name,
            "tradeassembly.broker.connect" | "tradeassembly.broker.verify"
        );
        if broker_onboarding_call && self.invocation_principal.is_none() {
            return mcp::tool_error(
                name,
                "authority_context_required",
                "Broker setup requires an authenticated MCP session.",
                None,
            );
        }
        if name.starts_with("tradeassembly.broker.") {
            if let Some(code) = broker_onboarding::validate_mcp_arguments(name, &arguments) {
                return mcp::tool_error(name, code, "Broker setup requires an explicit account mode and idempotency key; identity comes from sign-in. Enter credentials only in the browser connection screen.", None);
            }
        }
        if broker_onboarding_call {
            if let Some(code) = broker_onboarding::validate_requested_mode_authority(&arguments) {
                return mcp::tool_error(
                    name,
                    code,
                    "Broker setup account mode conflicts with the requested mode.",
                    None,
                );
            }
        }
        if name == "tradeassembly.strategy.version.publish" {
            if let Some(error) = validate_mcp_publish_arguments(name, &arguments) {
                return error;
            }
        }
        if self.agent_mcp_execution_context.is_none() {
            if let Some(code) = agent_deployment::validate_mcp_arguments(name, &arguments) {
                return mcp::tool_error(
                    name,
                    code,
                    "Agent deployment lifecycle mutations require explicit authority and idempotency.",
                    None,
                );
            }
        }
        let arguments = self.enrich_mcp_arguments(name, arguments);
        let trusted_runner_mode = self
            .agent_mcp_execution_context
            .as_ref()
            .map(|context| context.mode().to_string());
        let arguments = self
            .invocation_principal
            .as_ref()
            .map_or(arguments.clone(), |principal| {
                bind_authenticated_invocation_value(
                    arguments,
                    principal,
                    "mcp",
                    self.invocation_actor_kind,
                    agent_deployment::preserve_requested_mcp_mode(name)
                        || matches!(
                            name,
                            "tradeassembly.broker.connect"
                                | "tradeassembly.broker.verify"
                                | "tradeassembly.execution.config.save"
                        ),
                    trusted_runner_mode.as_deref(),
                )
            });
        let arguments = if let Some(context) = &self.agent_mcp_execution_context {
            match crate::agent_runner::bind_mcp_execution_context(
                self.runtime.as_ref(),
                context,
                name,
                arguments,
            ) {
                Ok(arguments) => arguments,
                Err(code) => {
                    return mcp::tool_error(
                        name,
                        &code,
                        "Runner-scoped MCP capability rejected this call.",
                        None,
                    );
                }
            }
        } else {
            arguments
        };
        if broker_onboarding_call {
            if let Some(code) = broker_onboarding::validate_mcp_arguments(name, &arguments) {
                return mcp::tool_error(name, code, "Broker setup requires explicit account mode and authority. Enter credentials only in the browser connection screen.", None);
            }
        }
        if let Some(code) = agent_deployment::validate_mcp_arguments(name, &arguments) {
            return mcp::tool_error(
                name,
                code,
                "Agent deployment lifecycle mutations require explicit authority and idempotency.",
                None,
            );
        }
        if let Some(path) = mcp_object_authorization_route(name) {
            if let Some(response) =
                object_authorization::authorize_private_request(self, "POST", path, &arguments)
            {
                return response.body;
            }
        }
        // Export remains a protected action, but an implicit retry must observe
        // a newly completed run instead of replaying a cached pre-completion error.
        // Explicit caller idempotency keys keep their existing replay semantics.
        let mut arguments = arguments;
        if name == "tradeassembly.backtest.export"
            && arguments.get("idempotency_key").is_none()
            && arguments.get("idempotencyKey").is_none()
        {
            let run_id = arguments["backtest_id"].as_str().unwrap_or_default();
            if let Ok(Some(run)) = self.runtime.backtests.get_run(run_id) {
                let kind = arguments["export_kind"].as_str().unwrap_or("reportJson");
                let binding = json!([run.run_id, run.sequence, run.result_hash, kind]);
                if let Ok(bytes) = serde_json_canonicalizer::to_vec(&binding) {
                    arguments["idempotency_key"] =
                        json!(format!("backtest-export:{:x}", Sha256::digest(bytes)));
                }
            }
        }
        let command_envelope = match control_plane::mcp_envelope(name, &arguments)
            .map(|envelope| inherit_trusted_correlation(envelope, &arguments))
        {
            Ok(envelope) => match control_plane::prepare_command(
                self.runtime.storage.as_ref(),
                self.runtime.finance_authority.as_ref(),
                envelope.clone(),
            ) {
                Ok(record) if record.duplicate => {
                    return record
                        .response_status
                        .map(|status| {
                            record.response_body.clone().map_or_else(
                                || mcp_duplicate_response(name, status),
                                |body| mcp_duplicate_response_with_body(name, status, body),
                            )
                        })
                        .unwrap_or_else(|| {
                            mcp::tool_error(
                                name,
                                "idempotency_in_progress",
                                "MCP command is already in progress for this idempotency key.",
                                None,
                            )
                        });
                }
                Ok(_) => Some(envelope),
                Err(error) if error == "idempotency_conflict" => {
                    return mcp::tool_error(
                        name,
                        "idempotency_conflict",
                        "MCP command idempotency key was reused with a different payload.",
                        None,
                    );
                }
                Err(error) if error.starts_with("finance_authority:") => {
                    let details = if name == "tradeassembly.execution.activate" {
                        Some(execution::activation_blocked_preflight(
                            self,
                            arguments.clone(),
                        ))
                    } else {
                        None
                    };
                    return mcp::tool_error(
                        name,
                        "finance_authority_denied",
                        "MCP command failed TradeAssembly finance authority checks.",
                        details,
                    );
                }
                Err(_) => None,
            },
            Err(_) => None,
        };
        let arguments = command_envelope
            .as_ref()
            .map(|envelope| with_control_plane_metadata(arguments.clone(), envelope))
            .unwrap_or(arguments);
        let payload = match name {
            "tradeassembly.execution.live_mandate.issue" => {
                live_authorization::issue(self, arguments.clone()).body
            }
            "tradeassembly.execution.live_mandate.revoke" => {
                live_authorization::revoke(self, arguments.clone()).body
            }
            "tradeassembly.execution.live_mandate.status" => {
                live_authorization::status(self, arguments.clone()).body
            }
            "tradeassembly.broker.connect" => broker_onboarding::connect(self, arguments.clone()),
            "tradeassembly.broker.status" => broker_onboarding::status(self, arguments.clone()),
            "tradeassembly.broker.verify" => broker_onboarding::verify(self, arguments.clone()),
            "tradeassembly.health" => {
                let mut body = self.handle_http("GET", "/health", json!({})).body;
                body["ok"] = json!(true);
                body["transport"] = json!("stdio");
                body["db"] = json!(arguments["db"].as_str().unwrap_or(self.db()));
                body["studioBaseUrl"] = json!(arguments["studio_base_url"]
                    .as_str()
                    .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL));
                body["noAdviceNotice"] = json!(LEGAL_BOUNDARY);
                body
            }
            "tradeassembly.setup.status" => {
                json!({"ok": true, "db": self.db, "studioBaseUrl": mcp::DEFAULT_STUDIO_BASE_URL, "credentialPosture": "local/customer-managed"})
            }
            "tradeassembly.account.status" => auth::local_account_status(
                arguments["profile"].as_str().unwrap_or("local"),
                arguments["studio_base_url"]
                    .as_str()
                    .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL),
            ),
            "tradeassembly.account.login" => auth::local_account_login(
                arguments["profile"].as_str().unwrap_or("local"),
                arguments["studio_base_url"]
                    .as_str()
                    .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL),
            ),
            "tradeassembly.install.claim" => auth::local_install_claim(
                arguments["profile"].as_str().unwrap_or("local"),
                arguments["install_id"].as_str(),
                arguments["studio_base_url"]
                    .as_str()
                    .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL),
            ),
            "tradeassembly.telemetry.emit" => auth::local_telemetry_emit(
                arguments["profile"].as_str().unwrap_or("local"),
                arguments["event"]
                    .as_str()
                    .or_else(|| arguments["event_type"].as_str())
                    .unwrap_or("first_backtest"),
                arguments["install_id"].as_str(),
                arguments["studio_base_url"]
                    .as_str()
                    .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL),
            ),
            "tradeassembly.telemetry.opt_out" => auth::local_telemetry_opt_out(
                arguments["profile"].as_str().unwrap_or("local"),
                arguments["telemetry_opt_out"].as_bool().unwrap_or(true),
                arguments["studio_base_url"]
                    .as_str()
                    .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL),
            ),
            "tradeassembly.telemetry.replay" => auth::local_telemetry_replay(
                arguments["profile"].as_str().unwrap_or("local"),
                arguments["studio_base_url"]
                    .as_str()
                    .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL),
            ),
            "tradeassembly.studio.link" => {
                json!({"url": format!("{}{}", mcp::DEFAULT_STUDIO_BASE_URL, arguments["path"].as_str().unwrap_or("/"))})
            }
            "studio.deployment.create" => agent_deployment::create(self, arguments),
            "studio.deployment.start" => agent_deployment::start(self, arguments),
            "studio.deployment.pause" => agent_deployment::pause(self, arguments),
            "studio.deployment.stop" => agent_deployment::stop(self, arguments),
            "studio.deployment.inspect" => agent_deployment::inspect(self, arguments),
            "studio.agent_run.inspect" => agent_deployment::inspect_run(self, arguments),
            "studio.agent_run.events" => agent_deployment::events(self, arguments),
            "studio.agent_run.health" => agent_deployment::health(self, arguments),
            "studio.agent_run.recover" => agent_deployment::recover(self, arguments),
            "studio.execution.external_receipt.append" => {
                external_broker_evidence::append(self, arguments)
            }
            "studio.execution.external_receipt.inspect" => {
                external_broker_evidence::inspect(self, arguments)
            }
            "studio.execution.external_receipt.reconcile" => {
                external_broker_evidence::reconcile(self, arguments)
            }
            "tradeassembly.sightline.session_state" => sightline::session_state(self, arguments),
            "tradeassembly.sightline.get_current_selection" => {
                sightline::current_selection_context(self, arguments)
            }
            "tradeassembly.sightline.get_context" => sightline::get_context(self, arguments),
            "tradeassembly.sightline.create_proposal" => {
                sightline::create_proposal(self, arguments)
            }
            "tradeassembly.sightline.request_approval" => {
                sightline::request_approval(self, arguments)
            }
            "tradeassembly.sightline.navigate_surface" => {
                sightline::request_navigation(self, arguments)
            }
            "tradeassembly.sightline.list_events" => sightline::list_events(self, arguments),
            "tradeassembly.strategy.list" => json!({"ok": true, "strategies": self.strategies()}),
            "tradeassembly.strategy.get" => self.strategy_detail(
                arguments["strategy_id"]
                    .as_str()
                    .unwrap_or("strat_local_btc_demo"),
            ),
            "tradeassembly.journal.list" => {
                self.handle_http_from_source("mcp", "GET", "/journal/events", arguments)
                    .body
            }
            "tradeassembly.journal.export" => {
                self.handle_http_from_source("mcp", "GET", "/journal/export", arguments)
                    .body
            }
            "tradeassembly.journal.replay" => {
                self.handle_http_from_source("mcp", "POST", "/journal/replay-report", arguments)
                    .body
            }
            "tradeassembly.strategy.schema" => json!({
                "ok": true,
                "schema": spec::strategy_spec_schema(),
                "evaluators": [crate::strategy_kernel::portfolio_program::discovery()],
                "guidance": "Encode only owner-supplied strategy rules. Blank drafts are incomplete. Validate before requesting owner publication; publication does not activate execution."
            }),
            "tradeassembly.strategy.create" => self.create_strategy(arguments),
            "tradeassembly.strategy.save_draft" | "tradeassembly.strategy.draft.save" => {
                self.save_builder_draft(arguments)
            }
            "tradeassembly.strategy.draft.select_node" => {
                self.select_strategy_semantic_node(arguments)
            }
            "tradeassembly.strategy.draft.propose_patch" => self.propose_strategy_patch(arguments),
            "tradeassembly.strategy.draft.review_patch"
            | "tradeassembly.strategy.draft.apply_patch" => {
                let mut args = arguments;
                args["actor"] =
                    json!({"kind": "agent", "id": "mcp.agent", "principalUser": "user.local"});
                self.review_strategy_patch(args)
            }
            "tradeassembly.strategy.version.publish" => {
                // Authentication identifies the owner, not a human approval.
                // Neither caller-supplied actor fields nor the authenticated
                // principal may turn an MCP invocation into user publication.
                let response = mcp::tool_error(
                    name,
                    "agent_cannot_publish_strategy_version",
                    "Present the exact draft to the owner for review. Publication requires an explicit owner CLI acknowledgment and does not activate execution.",
                    Some(json!({"nextAction": {
                        "action": "owner_strategy_publication",
                        "requiresOwnerAcknowledgement": true,
                        "reuseCurrentInstallation": true,
                        "commandArguments": [
                            "strategy", "publish", arguments["strategy_id"],
                            "--expected-draft-hash", arguments["expected_draft_hash"],
                            "--idempotency-key", format!("owner-publication:{}", arguments["expected_draft_hash"].as_str().unwrap_or_default()),
                            "--acknowledge-publication"
                        ]
                    }})),
                );
                return self.complete_mcp_command_or_error(name, &command_envelope, 403, response);
            }
            "tradeassembly.strategy.validate" => {
                if let Some(payload) = arguments.get("spec") {
                    let report = spec::validate_strategy_spec_report(payload);
                    json!({"ok": true, "valid": report.valid, "compilation": crate::strategy_kernel::portfolio_program::compilation_report(payload, report.valid), "report": report})
                } else if arguments
                    .get("strategy_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !id.is_empty())
                {
                    self.validate_builder_draft(arguments)
                } else {
                    return mcp::tool_error(name, "strategy_validation_input_required", "Supply spec JSON directly or strategy_id for an owned saved draft. File paths are not loaded by this tool.", None);
                }
            }
            "tradeassembly.plugin.list" => {
                json!({"ok": true, "workspacePrimitive": "plugins", "plugins": self.plugins_providers()["plugins"].clone()})
            }
            "tradeassembly.plugin.capability_resolve" => {
                providers::resolve_capability(self, providers::requirement_from_body(&arguments))
            }
            "tradeassembly.plugin.capability_graph_resolve" => {
                capability_graph::resolve(self, arguments)
            }
            "tradeassembly.plugin.capability_revision_save" => {
                capability_graph::save_revision(self, arguments)
            }
            "tradeassembly.plugin.capability_revision_get" => capability_graph::get_revision(
                self,
                arguments["revision_id"].as_str().unwrap_or_default(),
            ),
            "tradeassembly.plugin.capability_revision_check" => {
                capability_graph::check_current(self, arguments)
            }
            "tradeassembly.provider.list" => json!({"ok": true, "providers": self.providers()}),
            "tradeassembly.plugin.capability_matrix"
            | "tradeassembly.provider.capability_matrix" => {
                match explicit_provider_ref_from(&arguments) {
                    Some(provider_ref) => providers::capability_matrix(
                        self,
                        &provider_ref,
                        arguments.get("manifest").cloned(),
                    ),
                    None => providers::capability_matrix_ref_required(),
                }
            }
            "tradeassembly.provider.pack_compatibility" => {
                match explicit_provider_ref_from(&arguments) {
                    Some(provider_ref) => providers::capability_matrix(
                        self,
                        &provider_ref,
                        arguments.get("manifest").cloned(),
                    ),
                    None => providers::capability_matrix_ref_required(),
                }
            }
            "tradeassembly.credential.status" => self.credential_status(
                arguments["provider_ref"]
                    .as_str()
                    .unwrap_or("plugin-instance"),
            ),
            "tradeassembly.credential.test" => {
                let provider_ref = arguments["provider_ref"]
                    .as_str()
                    .unwrap_or("plugin-instance")
                    .to_string();
                api_result(credentials::test(self, &provider_ref, arguments))
            }
            "tradeassembly.plugin.oauth.start" => {
                self.dispatch_http("POST", "/plugins/oauth/start", arguments)
                    .body
            }
            "tradeassembly.plugin.oauth.status" => {
                self.dispatch_http("POST", "/plugins/oauth/status", arguments)
                    .body
            }
            "tradeassembly.plugin.oauth.disconnect" => {
                self.dispatch_http("POST", "/plugins/oauth/disconnect", arguments)
                    .body
            }
            "tradeassembly.plugin.status" => self.plugins_providers(),
            "tradeassembly.indicator.status" => json!({"ok": true, "indicators": ["sma", "rsi"]}),
            "tradeassembly.backtest.run" => {
                backtest_lifecycle::create(self, nested_graphql_request(&arguments)).body
            }
            "tradeassembly.backtest.get" => {
                backtest_lifecycle::get(self, arguments["run_id"].as_str().unwrap_or_default()).body
            }
            "tradeassembly.backtest.report" => {
                backtest_lifecycle::report(
                    self,
                    arguments["backtest_id"]
                        .as_str()
                        .or_else(|| arguments["run_id"].as_str())
                        .unwrap_or_default(),
                )
                .body
            }
            "tradeassembly.backtest.export" => {
                backtest_lifecycle::export(
                    self,
                    arguments["backtest_id"].as_str().unwrap_or_default(),
                    arguments["export_kind"].as_str().unwrap_or("reportJson"),
                )
                .body
            }
            "tradeassembly.backtest.list" => backtest_lifecycle::list(self).body,
            "tradeassembly.backtest.cancel" => {
                let run_id = arguments["run_id"].as_str().unwrap_or_default().to_string();
                backtest_lifecycle::cancel(self, &run_id, arguments).body
            }
            "tradeassembly.backtest.retry" => {
                let run_id = arguments["run_id"].as_str().unwrap_or_default().to_string();
                backtest_lifecycle::retry(self, &run_id, arguments).body
            }
            "tradeassembly.backtest.process" => {
                backtest_lifecycle::process_request(
                    self,
                    arguments["worker"]
                        .as_str()
                        .unwrap_or("local-backtest-worker"),
                    arguments["run_id"].as_str(),
                )
                .body
            }
            "tradeassembly.backtest.replay" => {
                backtest_lifecycle::replay(
                    self,
                    arguments["run_id"]
                        .as_str()
                        .or_else(|| arguments["backtest_id"].as_str())
                        .unwrap_or_default(),
                )
                .body
            }
            "tradeassembly.robustness.run" => {
                robustness::create(self, nested_graphql_request(&arguments)).body
            }
            "tradeassembly.robustness.list" => robustness::list(self).body,
            "tradeassembly.robustness.get" => {
                robustness::get(self, arguments["run_id"].as_str().unwrap_or_default()).body
            }
            "tradeassembly.robustness.process" => {
                robustness::process(
                    self,
                    arguments["worker"]
                        .as_str()
                        .unwrap_or("local-robustness-worker"),
                )
                .body
            }
            "tradeassembly.robustness.cancel" => {
                let run_id = arguments["run_id"].as_str().unwrap_or_default().to_string();
                robustness::cancel(self, &run_id, arguments).body
            }
            "tradeassembly.robustness.retry" => {
                let run_id = arguments["run_id"].as_str().unwrap_or_default().to_string();
                robustness::retry(self, &run_id, arguments).body
            }
            "tradeassembly.dataset_ingestion.create" => {
                self.dispatch_http("POST", "/dataset-ingestions", arguments)
                    .body
            }
            "tradeassembly.dataset_ingestion.list" => {
                self.dispatch_http("GET", "/dataset-ingestions", arguments)
                    .body
            }
            "tradeassembly.dataset_ingestion.get" | "tradeassembly.dataset_ingestion.status" => {
                self.dispatch_http(
                    "GET",
                    &format!(
                        "/dataset-ingestions/{}",
                        arguments["ingestion_id"].as_str().unwrap_or_default()
                    ),
                    arguments,
                )
                .body
            }
            "tradeassembly.dataset_ingestion.cancel" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/dataset-ingestions/{}/cancel",
                        arguments["ingestion_id"].as_str().unwrap_or_default()
                    ),
                    arguments,
                )
                .body
            }
            "tradeassembly.dataset_ingestion.verify" => {
                self.dispatch_http(
                    "POST",
                    &format!(
                        "/dataset-ingestions/{}/verify",
                        arguments["ingestion_id"].as_str().unwrap_or_default()
                    ),
                    arguments,
                )
                .body
            }
            "tradeassembly.report.envelope" => {
                let kind = arguments
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("backtest");
                if kind == "scenario_valuation" {
                    json!({
                        "ok": false,
                        "error": {
                            "code": "scenario_valuation_requires_durable_derivatives_analysis",
                            "message": "Create or read a durable derivatives analysis bound to an exact completed source run."
                        },
                        "noAdvice": LEGAL_BOUNDARY
                    })
                } else if kind == "monte_carlo" {
                    json!({
                        "ok": false,
                        "error": {
                            "code": "monte_carlo_requires_durable_robustness_run",
                            "message": "Create or read a durable robustness run bound to exact completed backtest evidence."
                        },
                        "noAdvice": LEGAL_BOUNDARY
                    })
                } else if kind == "portfolio_risk" {
                    let request = portfolio_risk_body_from(&arguments);
                    let snapshot = self.portfolio_risk_overlay(request);
                    report_envelope::portfolio_risk_surface(
                        snapshot,
                        &strategy_id_from(&arguments),
                        arguments["studio_base_url"]
                            .as_str()
                            .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL),
                        self.db(),
                    )
                } else if kind == "fill_quality" {
                    self.fill_quality_analysis(fill_quality_body_from(&arguments))
                } else if kind == "lifecycle_calendar" {
                    self.lifecycle_calendar(lifecycle_calendar_body_from(&arguments))
                } else {
                    report_envelope::report_envelope(
                        kind,
                        &strategy_id_from(&arguments),
                        arguments["backtest_id"]
                            .as_str()
                            .unwrap_or("backtest-local"),
                        arguments["studio_base_url"]
                            .as_str()
                            .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL),
                    )
                }
            }
            "tradeassembly.scenario.valuation" => derivatives::create(self, arguments).body,
            "tradeassembly.monte_carlo.run" => robustness::create(self, arguments).body,
            "tradeassembly.monte_carlo.status" => {
                robustness::get(
                    self,
                    arguments["run_id"]
                        .as_str()
                        .or_else(|| arguments["runId"].as_str())
                        .unwrap_or_default(),
                )
                .body
            }
            "tradeassembly.monte_carlo.report" => {
                robustness::report(
                    self,
                    arguments["run_id"]
                        .as_str()
                        .or_else(|| arguments["runId"].as_str())
                        .unwrap_or_default(),
                )
                .body
            }
            "tradeassembly.monte_carlo.replay" => {
                robustness::replay(
                    self,
                    arguments["run_id"]
                        .as_str()
                        .or_else(|| arguments["runId"].as_str())
                        .unwrap_or_default(),
                )
                .body
            }
            "tradeassembly.monte_carlo.export" => {
                robustness::export(
                    self,
                    arguments["run_id"]
                        .as_str()
                        .or_else(|| arguments["runId"].as_str())
                        .unwrap_or_default(),
                    arguments["format"].as_str().unwrap_or("json"),
                )
                .body
            }
            "tradeassembly.portfolio_risk.run"
            | "tradeassembly.portfolio_risk.report"
            | "tradeassembly.portfolio_risk.replay"
            | "tradeassembly.portfolio_risk.export" => {
                let request = portfolio_risk_body_from(&arguments);
                let snapshot = self.portfolio_risk_overlay(request);
                report_envelope::portfolio_risk_surface(
                    snapshot,
                    &strategy_id_from(&arguments),
                    arguments["studio_base_url"]
                        .as_str()
                        .unwrap_or(mcp::DEFAULT_STUDIO_BASE_URL),
                    self.db(),
                )
            }
            "tradeassembly.fill_quality.inspect"
            | "tradeassembly.fill_quality.report"
            | "tradeassembly.fill_quality.replay"
            | "tradeassembly.fill_quality.export" => {
                self.fill_quality_analysis(fill_quality_body_from(&arguments))
            }
            "tradeassembly.attribution_journal.review"
            | "tradeassembly.attribution_journal.report"
            | "tradeassembly.attribution_journal.inspect"
            | "tradeassembly.attribution_journal.replay"
            | "tradeassembly.attribution_journal.export" => {
                self.attribution_journal_analysis(attribution_journal_body_from(&arguments))
            }
            "tradeassembly.lifecycle_calendar.list"
            | "tradeassembly.lifecycle_calendar.inspect"
            | "tradeassembly.lifecycle_calendar.export"
            | "tradeassembly.lifecycle_calendar.replay" => {
                self.lifecycle_calendar(lifecycle_calendar_body_from(&arguments))
            }
            "tradeassembly.research_notebook.create"
            | "tradeassembly.research_notebook.compose" => {
                self.research_notebook_operation("compose", research_notebook_body_from(&arguments))
            }
            "tradeassembly.research_notebook.attach" => {
                self.research_notebook_operation("attach", research_notebook_body_from(&arguments))
            }
            "tradeassembly.research_notebook.list" => {
                self.research_notebook_operation("list", research_notebook_body_from(&arguments))
            }
            "tradeassembly.research_notebook.inspect" => {
                self.research_notebook_operation("inspect", research_notebook_body_from(&arguments))
            }
            "tradeassembly.research_notebook.export" => {
                self.research_notebook_operation("export", research_notebook_body_from(&arguments))
            }
            "tradeassembly.research_notebook.replay" => {
                self.research_notebook_operation("replay", research_notebook_body_from(&arguments))
            }
            "tradeassembly.execution.config.save" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-configs/save",
                    arguments,
                )
                .body
            }
            "tradeassembly.execution.readiness" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-configs/activation-readiness",
                    arguments,
                )
                .body
            }
            "tradeassembly.execution.activate" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-activations/activate",
                    arguments,
                )
                .body
            }
            "tradeassembly.execution.control" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-activations/control",
                    arguments,
                )
                .body
            }
            "tradeassembly.execution.deactivate" => {
                self.dispatch_http(
                    "POST",
                    "/product/strategy-execution-activations/deactivate",
                    arguments,
                )
                .body
            }
            "tradeassembly.execution.scheduler.start" => {
                self.dispatch_http("POST", "/product/scheduler/start", arguments)
                    .body
            }
            "tradeassembly.execution.scheduler.run" => {
                self.dispatch_http("POST", "/scheduler/run", arguments).body
            }
            "tradeassembly.execution.scheduler.stop" => {
                self.dispatch_http("POST", "/product/scheduler/stop", arguments)
                    .body
            }
            "tradeassembly.execution.status" => {
                let strategy_id = strategy_id_from(&arguments);
                json!({"ok": true, "workspace": self.execution_workspace(&strategy_id, &arguments), "runCenter": execution::run_center(self)})
            }
            "tradeassembly.execution.run" => {
                if mcp_submission_requested(&arguments)
                    && !mcp_submission_has_explicit_authority(&arguments)
                {
                    let response = mcp::tool_error(
                        name,
                        "mcp_order_submission_disabled",
                        "MCP order submission requires explicit allow_mcp_order_submission, authority_context, idempotency_key, and account_mode.",
                        Some(json!({
                            "submit_orders": arguments["submit_orders"].as_bool().unwrap_or(false),
                            "submit_exit": arguments["submit_exit"].as_bool().unwrap_or(false),
                        })),
                    );
                    return self.complete_mcp_command_or_error(
                        name,
                        &command_envelope,
                        403,
                        response,
                    );
                }
                self.run_once(mcp_execution_run_body(&arguments))
            }
            "tradeassembly.execution.replay" => {
                let strategy_id = strategy_id_from(&arguments);
                let workspace = self.execution_workspace(&strategy_id, &arguments);
                let audit = self
                    .runtime()
                    .finance_authority
                    .audit(self.runtime().storage.as_ref());
                json!({
                    "ok": true,
                    "workspace": workspace.clone(),
                    "replay": workspace["replay"].clone(),
                    "execution": workspace["execution"].clone(),
                    "apfAudit": audit,
                    "counts": {
                        "events": workspace["events"].as_array().map(Vec::len).unwrap_or(0),
                        "orders": workspace["orders"].as_array().map(Vec::len).unwrap_or(0),
                    },
                })
            }
            _ => {
                let payload = mcp::call_tool(name, arguments);
                let status = if payload["isError"].as_bool().unwrap_or(false) {
                    400
                } else {
                    200
                };
                return self.complete_mcp_command_or_error(
                    name,
                    &command_envelope,
                    status,
                    payload,
                );
            }
        };
        if matches!(
            name,
            "tradeassembly.scenario.valuation"
                | "tradeassembly.monte_carlo.run"
                | "tradeassembly.monte_carlo.status"
                | "tradeassembly.monte_carlo.report"
                | "tradeassembly.monte_carlo.replay"
                | "tradeassembly.monte_carlo.export"
        ) || name.starts_with("tradeassembly.robustness.")
            || name.starts_with("tradeassembly.derivatives.")
            || name.starts_with("tradeassembly.comparison.")
        {
            if let Some(code) = service_error_code(&payload) {
                let response = mcp::tool_error(
                    name,
                    &code,
                    "Research command failed closed.",
                    Some(payload),
                );
                return self.complete_mcp_command_or_error(name, &command_envelope, 400, response);
            }
        }
        if name.starts_with("studio.execution.external_receipt.")
            || name.starts_with("tradeassembly.broker.")
        {
            if let Some(code) = service_error_code(&payload) {
                let response = mcp::tool_error(
                    name,
                    &code,
                    "External broker receipt command failed closed.",
                    Some(payload),
                );
                return self.complete_mcp_command_or_error(name, &command_envelope, 400, response);
            }
        }
        if payload["error"]["retryable"].as_bool() == Some(true) {
            let code = service_error_code(&payload)
                .unwrap_or_else(|| "activation_dependency_unavailable".to_string());
            let response = mcp::tool_error(
                name,
                &code,
                "Execution dependency is temporarily unavailable.",
                None,
            );
            return self.complete_mcp_command_or_error(name, &command_envelope, 503, response);
        }
        if name.starts_with("studio.deployment.") || name.starts_with("studio.agent_run.") {
            if let Some(code) = service_error_code(&payload) {
                let response = mcp::tool_error(
                    name,
                    &code,
                    "Agent deployment lifecycle command failed closed.",
                    Some(payload),
                );
                return self.complete_mcp_command_or_error(name, &command_envelope, 400, response);
            }
        }
        if name.starts_with("tradeassembly.dataset_ingestion.") {
            let status = dataset_ingestion_status(&payload, 200);
            if status >= 400 {
                let code = service_error_code(&payload)
                    .unwrap_or_else(|| "dataset_ingestion_failed".to_string());
                let response = mcp::tool_error(
                    name,
                    &code,
                    "Historical dataset ingestion command failed.",
                    Some(payload),
                );
                return self.complete_mcp_command_or_error(
                    name,
                    &command_envelope,
                    status,
                    response,
                );
            }
        }
        if name.starts_with("tradeassembly.journal.") {
            if let Some(code) = service_error_code(&payload) {
                return mcp::tool_error(name, &code, "Journal access failed.", None);
            }
        }
        let response = mcp::call_tool_with_payload(name, payload);
        self.complete_mcp_command_or_error(name, &command_envelope, 200, response)
    }

    fn workspace(&self) -> Value {
        workspace::workspace(self)
    }

    fn workspace_shell(&self) -> Value {
        workspace::workspace_shell(self)
    }

    fn strategies(&self) -> Vec<Value> {
        strategy::strategies(self)
    }

    fn create_strategy(&self, body: Value) -> Value {
        strategy::create_strategy(self, body)
    }

    fn strategy_value(&self, id: &str) -> Value {
        strategy::strategy_value(self, id)
    }

    fn strategy_detail(&self, id: &str) -> Value {
        strategy::strategy_detail(self, id)
    }

    fn save_builder_draft(&self, body: Value) -> Value {
        strategy::save_builder_draft(self, body)
    }

    fn validate_builder_draft(&self, body: Value) -> Value {
        strategy::validate_builder_draft(self, body)
    }

    fn publish_strategy(&self, body: Value) -> Value {
        strategy::publish_strategy(self, body)
    }

    fn select_strategy_semantic_node(&self, body: Value) -> Value {
        strategy::select_strategy_semantic_node(self, body)
    }

    fn propose_strategy_patch(&self, body: Value) -> Value {
        strategy::propose_strategy_patch(self, body)
    }

    fn review_strategy_patch(&self, body: Value) -> Value {
        strategy::review_strategy_patch(self, body)
    }

    fn duplicate_strategy(&self, body: Value) -> Value {
        strategy::duplicate_strategy(self, body)
    }

    fn set_strategy_status(&self, id: &str, status: &str) -> Value {
        strategy::set_strategy_status(self, id, status)
    }

    fn strategy_versions(&self, id: &str) -> Vec<Value> {
        strategy::strategy_versions(self, id)
    }

    fn providers(&self) -> Vec<Value> {
        providers::list(self)
    }

    fn credential_status(&self, provider_ref: &str) -> Value {
        credentials::status(self, provider_ref)
    }

    fn plugins_providers(&self) -> Value {
        plugin_lifecycle::workspace(self)
    }

    fn run_once(&self, body: Value) -> Value {
        orders::run_once(self, body)
    }

    fn demo_run(&self, id: &str) -> Value {
        demos::DemoRunner
            .run(id)
            .map(|result| serde_json::to_value(result).unwrap_or_else(|_| json!({})))
            .unwrap_or_else(|error| json!({"ok": false, "error": error}))
    }

    pub(crate) fn seed_btc_demo(&self) -> Value {
        strategy::seed_default_version(self, "strat_local_btc_demo");
        self.demo_run("local-simbroker-paper-btc")
    }

    fn builder_state(&self, strategy_id: &str) -> Value {
        workspace::builder_state(self, strategy_id)
    }

    fn research_workspace(&self, strategy_id: &str) -> Value {
        research::workspace(self, strategy_id)
    }

    fn execution_workspace(&self, strategy_id: &str, query: &Value) -> Value {
        let fill_quality_report =
            self.fill_quality_analysis(fill_quality_body_from(&json!({"strategyId": strategy_id})));
        let lifecycle_calendar = self.lifecycle_calendar(lifecycle_calendar_body_from(
            &json!({"strategyId": strategy_id}),
        ));
        let attribution_journal = self.attribution_journal_analysis(attribution_journal_body_from(
            &json!({"strategyId": strategy_id}),
        ));
        let mut workspace = execution::workspace(self, strategy_id, query);
        workspace["fillQualityReport"] = fill_quality_report.clone();
        workspace["lifecycleCalendar"] = lifecycle_calendar.clone();
        workspace["attributionJournal"] = attribution_journal.clone();
        workspace["execution"]["fillQualityReport"] = fill_quality_report;
        workspace["execution"]["lifecycleCalendar"] = lifecycle_calendar;
        workspace["execution"]["attributionJournal"] = attribution_journal;
        workspace
    }

    fn instrument_context(&self, strategy_id: &str) -> Value {
        json!({
            "strategyId": strategy_id,
            "strategy": self.strategy_value(strategy_id),
            "instrumentContext": {
                "schemaVersion": "tradeassembly.instrument_context.v1",
                "operation": "resolve",
                "canonicalId": "ul:crypto:btc-usd",
                "assetClass": "crypto",
                "instrument": {"canonicalId": "ul:crypto:btc-usd", "symbol": "BTC/USD", "assetClass": "crypto"},
                "aliases": ["BTC/USD", "BTCUSD"],
                "provenance": [{"providerRef": "local-data"}],
                "warnings": [],
                "notice": "Instrument context is identity metadata only.",
            },
        })
    }
}

fn validate_studio_origin(origin: &str) -> Result<(), String> {
    let parsed = url::Url::parse(origin.trim()).map_err(|_| "studio_origin_invalid".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
        || parsed.host_str().is_none()
    {
        return Err("studio_origin_invalid".to_string());
    }
    if parsed.scheme() == "http"
        && !matches!(
            parsed.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
        )
    {
        return Err("studio_origin_invalid".to_string());
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
struct TestDatabaseIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

static TEST_DATABASE_REGISTRY: OnceLock<Mutex<HashMap<PathBuf, TestDatabaseIdentity>>> =
    OnceLock::new();

fn test_database_path(db: &str) -> Result<PathBuf, String> {
    let supplied = Path::new(db);
    let path = if supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|_| "test_database_path_invalid")?
            .join(supplied)
    };
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| "test_database_unavailable")?;
    if metadata.file_type().is_symlink() {
        return Err("test_database_handoff_invalid".to_string());
    }
    Ok(path)
}

fn test_database_handoff_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.handoff", path.display()))
}

/// Test-only construction is allowed to create a fresh fixture and to reopen
/// the exact fixture it registered in this process. It must never become a
/// general-purpose database opener: an existing unregistered file is rejected
/// before migrations, seeding, or scheduler startup can touch it.
fn assert_test_database_is_owned(db: &str) {
    if db == ":memory:" {
        return;
    }

    let supplied_path = Path::new(db);
    let path = if supplied_path.is_absolute() {
        supplied_path.to_path_buf()
    } else {
        std::env::current_dir()
            .expect("test database path requires a current directory")
            .join(supplied_path)
    };

    let registry = TEST_DATABASE_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if !path.exists() {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|_| panic!("test database fixture directory is unavailable"));
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_or_else(|_| panic!("test database fixture could not be created"));
    }

    let symlink_metadata = std::fs::symlink_metadata(&path)
        .unwrap_or_else(|_| panic!("test database fixture cannot be inspected"));
    if symlink_metadata.file_type().is_symlink() {
        panic!("test database fixture symlink is not process-owned");
    }
    let metadata = std::fs::metadata(&path)
        .unwrap_or_else(|_| panic!("test database fixture cannot be inspected"));
    if metadata.len() != 0 && !registry.contains_key(&path) {
        panic!("test database factory refuses an existing unregistered database");
    }

    let identity = test_database_identity(&metadata);
    match registry.get(&path) {
        Some(expected) if expected == &identity => {}
        Some(_) => panic!("test database fixture identity changed"),
        None => {
            registry.insert(path, identity);
        }
    }
}

#[cfg(unix)]
fn test_database_identity(metadata: &std::fs::Metadata) -> TestDatabaseIdentity {
    use std::os::unix::fs::MetadataExt;
    TestDatabaseIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
fn test_database_identity(_metadata: &std::fs::Metadata) -> TestDatabaseIdentity {
    panic!("file-backed test database ownership is unsupported on this platform; use :memory:")
}

fn validate_mcp_publish_arguments(name: &str, arguments: &Value) -> Option<Value> {
    let strategy_id = arguments.get("strategy_id").and_then(Value::as_str);
    let expected_draft_hash = arguments.get("expected_draft_hash").and_then(Value::as_str);
    let actor = arguments.get("actor").and_then(Value::as_object);
    let actor_kind = actor
        .and_then(|value| value.get("kind"))
        .and_then(Value::as_str);
    let actor_id = actor
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str);

    let missing_or_invalid = strategy_id.is_none_or(|value| value.trim().is_empty())
        || expected_draft_hash.is_none_or(|value| value.trim().is_empty())
        || !matches!(actor_kind, Some("user" | "agent"))
        || actor_id.is_none_or(|value| value.trim().is_empty());
    missing_or_invalid.then(|| {
        mcp::tool_error(
            name,
            "invalid_tool_arguments",
            "Publishing requires strategy_id, expected_draft_hash, and an explicit user or agent actor with a nonempty id.",
            None,
        )
    })
}

pub const GRAPHQL_OPERATIONS: &[&str] = &[
    "LocalWorkspace",
    "StrategyLibrary",
    "StrategyHome",
    "StrategyDetail",
    "StrategyInstrumentContext",
    "StrategyVersionHistory",
    "StrategyBuilder",
    "StrategyContractMetadata",
    "StrategyResearch",
    "StrategyResearchWorkspace",
    "StrategyExecution",
    "StrategyExecutionWorkspace",
    "RunCenter",
    "RobustnessRuns",
    "RobustnessRun",
    "RobustnessReport",
    "ObserveStudioSetup",
    "DerivativesAnalyses",
    "DerivativesAnalysis",
    "DerivativesAnalysisExport",
    "ResearchComparisons",
    "ResearchComparison",
    "ResearchComparisonExport",
    "ResearchRollup",
    "DatasetIngestionList",
    "DatasetIngestionGet",
    "DatasetIngestionStatus",
    "AlertCenter",
    "DiscoverWorkspace",
    "StrategyShareWorkspace",
    "ConnectWorkspace",
    "Providers",
    "PluginsProviders",
    "PluginManifests",
    "PluginInstances",
    "PluginInstance",
    "Entitlements",
    "ResolveCapability",
    "ResolveCapabilityGraph",
    "SaveCapabilityGraphRevision",
    "CapabilityGraphRevision",
    "CheckCapabilityGraphRevision",
    "AdminPluginManifestRegistry",
    "StoreCredentials",
    "TestCredentials",
    "InstallPluginManifest",
    "InstallPluginPackage",
    "CreatePluginInstance",
    "ConfigurePluginInstance",
    "EnablePluginInstance",
    "DisablePluginInstance",
    "UpgradePluginInstance",
    "RollbackPluginInstance",
    "RemovePluginInstance",
    "StorePluginCredentials",
    "StartPluginOAuth",
    "PluginOAuthStatus",
    "DisconnectPluginOAuth",
    "RevokePluginCredentials",
    "RefreshPluginHealth",
    "RevokeEntitlementOverride",
    "CreateStrategy",
    "CreateStrategyAiDraft",
    "SaveBuilderDraft",
    "SelectStrategySemanticNode",
    "ProposeStrategyPatch",
    "ReviewStrategyPatch",
    "ApplyStrategyPatch",
    "SightlineSessionState",
    "PublishSightlineStrategyEditorSurface",
    "PublishSightlineStudioSurface",
    "SetSightlineSelection",
    "ShareSightlineSelection",
    "SightlineGetContext",
    "ConnectSightlineAgent",
    "SightlineUpdatePresence",
    "SightlineCreateProposal",
    "SightlineRequestApproval",
    "SightlineNavigateSurface",
    "SightlineFocusNode",
    "SightlineHighlightNode",
    "SightlineEvents",
    "ValidateBuilderDraft",
    "ValidateStrategyExpression",
    "PublishStrategy",
    "DuplicateStrategy",
    "ArchiveStrategy",
    "RestoreStrategy",
    "CompileStrategyPreview",
    "RunStrategyResearch",
    "CreateDatasetIngestion",
    "CancelDatasetIngestion",
    "VerifyDatasetIngestion",
    "RunScenarioValuation",
    "RunMonteCarlo",
    "RunPortfolioRisk",
    "PortfolioRiskReport",
    "FillQualityInspect",
    "FillQualityReport",
    "FillQualityReplay",
    "FillQualityExport",
    "AttributionJournalReview",
    "AttributionJournalReport",
    "AttributionJournalInspect",
    "AttributionJournalReplay",
    "AttributionJournalExport",
    "LifecycleCalendarList",
    "LifecycleCalendarInspect",
    "LifecycleCalendarExport",
    "LifecycleCalendarReplay",
    "ResearchNotebookCreate",
    "ResearchNotebookCompose",
    "ResearchNotebookAttach",
    "ResearchNotebookList",
    "ResearchNotebookInspect",
    "ResearchNotebookExport",
    "ResearchNotebookReplay",
    "MonteCarloStatus",
    "MonteCarloReport",
    "CreateResearchDataset",
    "QueueStrategyResearchJob",
    "StrategyResearchJobStatus",
    "PromoteStrategyResearchToPaper",
    "SaveExecutionConfig",
    "ActivationReadiness",
    "ActivateExecution",
    "ControlStrategyExecution",
    "DeactivateStrategyExecution",
    "UpdateProviderInstance",
    "EnableProviderInstance",
    "DisableProviderInstance",
    "AcknowledgeAlert",
    "CreateStrategyShareSnapshot",
    "RevokeStrategyShareSnapshot",
    "RunBacktest",
    "CreateRobustnessRun",
    "ProcessRobustnessRun",
    "CancelRobustnessRun",
    "RetryRobustnessRun",
    "ReplayRobustnessRun",
    "ExportRobustnessRun",
    "CreateDerivativesAnalysis",
    "CreateResearchComparison",
    "BacktestRuns",
    "BacktestRun",
    "BacktestReport",
    "CancelBacktest",
    "RetryBacktest",
    "ProcessBacktest",
    "ReplayBacktest",
    "BacktestReportExport",
    "RunResearchSweep",
    "CreateResearchUniverse",
    "RunStrategyOnce",
    "SchedulerStart",
    "SchedulerRun",
    "SchedulerStop",
    "ReconcileOrders",
];

pub fn api_result(body: Value) -> Value {
    json!({"ok": true, "body": body, "error": null})
}

fn compile_strategy_preview(body: Value) -> ServiceResponse {
    let spec_payload = strip_strategy_transport_metadata(body.get("spec").cloned().unwrap_or(body));
    let report = spec::validate_strategy_spec_report(&spec_payload);
    let boundary_violations = spec::strategy_spec_runtime_boundary_violations(&spec_payload);
    if !report.valid || !boundary_violations.is_empty() {
        return ServiceResponse::bad_request_with_details(
            "strategy_preview_invalid",
            json!({
                "report": report,
                "runtimeBoundaryViolations": boundary_violations,
            }),
        );
    }

    let symbols = spec_payload["signal_market_requirements"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|market| {
            market["selectors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|selector| selector["normalized_id"].as_str())
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    let signal_markets = spec_payload["signal_market_requirements"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|market| {
            json!({
                "requirementId": market["requirement_id"],
                "assetClass": market["asset_class"],
                "instrumentFamily": market["instrument_family"],
                "timeframes": market["timeframes"],
            })
        })
        .collect::<Vec<_>>();
    let trade_markets = spec_payload["trade_market_requirements"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let capability_requirements = spec_payload["capability_requirements"]["required"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|requirement| {
            json!({
                "requirementId": requirement["requirement_id"],
                "capability": requirement["capability"],
                "purpose": requirement["purpose"],
                "requiredFor": requirement["required_for"],
            })
        })
        .collect::<Vec<_>>();
    let order_policy = &spec_payload["stages"]["order_strategy"]["policy"];
    let order_preview = json!({
        "strategyId": spec_payload["strategy_id"],
        "strategyName": spec_payload["name"],
        "symbols": symbols,
        "signalMarkets": signal_markets,
        "tradeMarkets": trade_markets,
        "orderPolicy": {
            "allowedOrderTypes": order_policy["allowed_order_types"],
            "multiLegMode": order_policy["multi_leg_mode"],
            "timeInForce": order_policy["time_in_force"],
            "conditionalOrdersAllowed": order_policy["conditional_orders_allowed"],
        },
        "capabilityRequirements": capability_requirements,
        "specHash": report.spec_hash,
        "submit": false,
        "executable": false,
    });

    ServiceResponse::ok(api_result(json!({
        "orderPlanPreview": order_preview,
        "order_preview": order_preview,
        "validation": report,
        "noAdvice": LEGAL_BOUNDARY,
    })))
}

fn strip_strategy_transport_metadata(mut payload: Value) -> Value {
    if let Value::Object(map) = &mut payload {
        for transport_key in [
            "controlPlaneCommandId",
            "controlPlaneCorrelationId",
            "controlPlaneIdempotencyKey",
            "controlPlaneSourceInterface",
            "controlPlaneSideEffectClass",
            "controlPlaneExplicitAuthority",
            "accountMode",
            "authorityContext",
        ] {
            map.remove(transport_key);
        }
    }
    payload
}

pub fn graphql_schema_text() -> &'static str {
    r#"
scalar JSON
type Query {
  localWorkspace: JSON!
  strategyLibrary: JSON!
  strategyHome(strategyId: String!): JSON!
  strategyDetail(strategyId: String!): JSON!
  strategyInstrumentContext(strategyId: String!): JSON!
  strategyVersionHistory(strategyId: String!): JSON!
  strategyBuilder(strategyId: String!): JSON!
  strategyContractMetadata: JSON!
  strategyResearchWorkspace(strategyId: String!): JSON!
  strategyExecutionWorkspace(strategyId: String!): JSON!
  runCenter: JSON!
  researchRollup: JSON!
  datasetIngestionList: JSON!
  datasetIngestionGet(ingestionId: String!): JSON!
  datasetIngestionStatus(ingestionId: String!): JSON!
  verifyDatasetIngestion(ingestionId: String!): JSON!
  backtestRuns: JSON!
  backtestRun(runId: String!): JSON!
  backtestReport(runId: String!): JSON!
  robustnessRuns: JSON!
  robustnessRun(runId: String!): JSON!
  robustnessReport(runId: String!): JSON!
  derivativesAnalyses: JSON!
  derivativesAnalysis(analysisId: String!): JSON!
  derivativesAnalysisExport(analysisId: String!, format: String): JSON!
  researchComparisons: JSON!
  researchComparison(comparisonId: String!): JSON!
  researchComparisonExport(comparisonId: String!, format: String): JSON!
  alertCenter: JSON!
  discoverWorkspace: JSON!
  strategyShareWorkspace(strategyId: String!): JSON!
  pluginsProviders: JSON!
  pluginManifests: JSON!
  pluginInstances: JSON!
  pluginInstance(instanceRef: String!): JSON!
  entitlements: JSON!
  pluginCapabilityResolution(capability: String!, mode: String, instrumentType: String, operation: String, strategyId: String, pluginRef: String, accountRef: String, purpose: String): JSON!
  adminPluginManifestRegistry: JSON!
  connectWorkspace: JSON!
  sightlineSessionState(strategyId: String, surfaceId: String): JSON!
  sightlineGetContext(strategyId: String, surfaceId: String, selectionId: String, contextLevel: Int): JSON!
  sightlineEvents(strategyId: String, limit: Int): JSON!
  researchNotebookList(strategyId: String!): JSON!
  researchNotebookInspect(strategyId: String!, notebookId: String!): JSON!
}
type Mutation {
  createDatasetIngestion(request: JSON!): JSON!
  cancelDatasetIngestion(ingestionId: String!): JSON!
  storeCredentials(providerRef: String!, apiKey: String!, apiSecret: String!, paper: Boolean): JSON!
  testCredentials(providerRef: String!): JSON!
  installPluginManifest(manifest: JSON!, source: JSON!, integrity: JSON!, trust: JSON, idempotencyKey: String): JSON!
  installPluginPackage(source: JSON!, integrity: JSON!, trust: JSON, offline: Boolean, idempotencyKey: String): JSON!
  createPluginInstance(instanceRef: String!, pluginRef: String!, providerRef: String, accountRef: String, enabled: Boolean, configuration: JSON, idempotencyKey: String): JSON!
  configurePluginInstance(instanceRef: String!, configuration: JSON!, accountRef: String, idempotencyKey: String): JSON!
  enablePluginInstance(instanceRef: String!, idempotencyKey: String): JSON!
  disablePluginInstance(instanceRef: String!, idempotencyKey: String): JSON!
  upgradePluginInstance(instanceRef: String!, source: JSON!, integrity: JSON!, trust: JSON, offline: Boolean, idempotencyKey: String): JSON!
  rollbackPluginInstance(instanceRef: String!, idempotencyKey: String): JSON!
  removePluginInstance(instanceRef: String!, idempotencyKey: String): JSON!
  storePluginCredentials(instanceRef: String!, credentials: JSON!, idempotencyKey: String): JSON!
  startPluginOAuth(instanceRef: String!, environment: String!, scopes: [String!]!, idempotencyKey: String): JSON!
  pluginOAuthStatus(instanceRef: String!, connectionId: String!, idempotencyKey: String): JSON!
  disconnectPluginOAuth(instanceRef: String!, idempotencyKey: String): JSON!
  revokePluginCredentials(instanceRef: String!, idempotencyKey: String): JSON!
  refreshPluginHealth(instanceRef: String!, idempotencyKey: String): JSON!
  revokeEntitlementOverride(grantId: String!): JSON!
  createStrategy(mode: String!, templateId: String, name: String): JSON!
  createStrategyAiDraft(strategyId: String, brief: String!, market: String, horizon: String): JSON!
  saveBuilderDraft(strategyId: String!, name: String!, spec: JSON, capabilityBindings: JSON): JSON!
  selectStrategySemanticNode(strategyId: String!, nodeRef: String!): JSON!
  proposeStrategyPatch(strategyId: String!, expectedDraftHash: String, selectionRef: String, specPatch: JSON, summary: String, actor: JSON, purpose: String, evidenceRefs: JSON): JSON!
  reviewStrategyPatch(strategyId: String!, proposalId: String!, decision: String!, actor: JSON): JSON!
  applyStrategyPatch(strategyId: String!, proposalId: String!, actor: JSON): JSON!
  publishSightlineStrategyEditorSurface(strategyId: String!, route: String): JSON!
  publishSightlineStudioSurface(route: String!): JSON!
  setSightlineSelection(strategyId: String, surfaceId: String, nodeId: String, nodeRef: String, shared: Boolean, actor: JSON): JSON!
  shareSightlineSelection(strategyId: String!, selectionId: String, actor: JSON): JSON!
  connectSightlineAgent(strategyId: String, agentId: String, displayName: String, capabilities: JSON): JSON!
  sightlineUpdatePresence(strategyId: String, actor: JSON, state: String): JSON!
  sightlineCreateProposal(strategyId: String!, selectionId: String, summary: String, purpose: String, patch: JSON!, actor: JSON, sightlineRefs: JSON): JSON!
  sightlineRequestApproval(strategyId: String, action: String, authorityLevel: String, summary: String, actor: JSON): JSON!
  sightlineNavigateSurface(strategyId: String, surfaceId: String, route: String, nodeId: String, authorityLevel: String, actor: JSON): JSON!
  sightlineFocusNode(strategyId: String, surfaceId: String, nodeId: String, actor: JSON): JSON!
  sightlineHighlightNode(strategyId: String, surfaceId: String, nodeId: String, actor: JSON): JSON!
  validateBuilderDraft(spec: JSON!): JSON!
  validateStrategyExpression(expression: String!): JSON!
  publishStrategy(strategyId: String!, versionId: String, expectedDraftHash: String!): JSON!
  duplicateStrategy(strategyId: String!, name: String): JSON!
  archiveStrategy(strategyId: String!): JSON!
  restoreStrategy(strategyId: String!): JSON!
  compileStrategyPreview(strategyId: String!, spec: JSON): JSON!
  runStrategyResearch(strategyId: String!, engine: String): JSON!
  runScenarioValuation(request: JSON!): JSON!
  runMonteCarlo(request: JSON!): JSON!
  runPortfolioRisk(strategyId: String!, scopeKind: String): JSON!
  portfolioRiskReport(strategyId: String!, scopeKind: String): JSON!
  fillQualityInspect(strategyId: String!, analysisId: String): JSON!
  fillQualityReport(strategyId: String!, analysisId: String): JSON!
  fillQualityReplay(strategyId: String!, analysisId: String): JSON!
  fillQualityExport(strategyId: String!, analysisId: String): JSON!
  attributionJournalReview(strategyId: String!, reviewId: String): JSON!
  attributionJournalReport(strategyId: String!, reviewId: String): JSON!
  attributionJournalInspect(strategyId: String!, reviewId: String): JSON!
  attributionJournalReplay(strategyId: String!, reviewId: String): JSON!
  attributionJournalExport(strategyId: String!, reviewId: String): JSON!
  lifecycleCalendarList(strategyId: String!, timelineId: String): JSON!
  lifecycleCalendarInspect(strategyId: String!, timelineId: String): JSON!
  lifecycleCalendarExport(strategyId: String!, timelineId: String): JSON!
  lifecycleCalendarReplay(strategyId: String!, timelineId: String): JSON!
  researchNotebookCreate(strategyId: String!, title: String, artifactSelectors: JSON!): JSON!
  researchNotebookCompose(strategyId: String!, title: String, artifactSelectors: JSON!): JSON!
  researchNotebookAttach(strategyId: String!, title: String, artifactSelectors: JSON!): JSON!
  researchNotebookExport(strategyId: String!, notebookId: String!): JSON!
  researchNotebookReplay(strategyId: String!, notebookId: String!): JSON!
  monteCarloStatus(runId: String!): JSON!
  monteCarloReport(runId: String!): JSON!
  runBacktest(request: JSON, strategyId: String, strategyVersionId: String, datasetId: String, purpose: String, client: String, idempotencyKey: String): JSON!
  createDerivativesAnalysis(request: JSON!): JSON!
  createResearchComparison(request: JSON!): JSON!
  createRobustnessRun(request: JSON!): JSON!
  processRobustnessRun(runId: String!, worker: String): JSON!
  cancelRobustnessRun(runId: String!, idempotencyKey: String): JSON!
  retryRobustnessRun(runId: String!, idempotencyKey: String): JSON!
  replayRobustnessRun(runId: String!, idempotencyKey: String): JSON!
  exportRobustnessRun(runId: String!, format: String, idempotencyKey: String): JSON!
  observeStudioSetup(instanceRef: String!, mode: String!, idempotencyKey: String!): JSON!
  cancelBacktest(runId: String!, idempotencyKey: String): JSON!
  retryBacktest(runId: String!, idempotencyKey: String): JSON!
  processBacktest(runId: String!, worker: String): JSON!
  replayBacktest(runId: String!): JSON!
  backtestReportExport(backtestId: String!, exportKind: String): JSON!
  saveExecutionConfig(strategyId: String!, versionId: String, providerRef: String, dataProviderRef: String, mode: String, accountRef: String, dataAccountRef: String, riskLimits: JSON, params: JSON, celAllow: [String!]): JSON!
  activationReadiness(configId: String!): JSON!
  activateExecution(configId: String, strategyId: String, acknowledgementIds: [String!]): JSON!
  controlStrategyExecution(activationId: String!, action: String!, idempotencyKey: String): JSON!
  deactivateStrategyExecution(activationId: String!): JSON!
  schedulerRun(activationId: String, intervalSeconds: Int, maxCycles: Int): JSON!
}
"#
}

fn normalize_path(path: &str) -> String {
    path.split('?')
        .next()
        .unwrap_or(path)
        .trim_end_matches('/')
        .to_string()
}

fn dataset_ingestion_path_id(path: &str) -> &str {
    path.trim_start_matches("/dataset-ingestions/")
        .split('/')
        .next()
        .unwrap_or_default()
}

fn dataset_ingestion_response(body: Value, success_status: u16) -> ServiceResponse {
    let status = dataset_ingestion_status(&body, success_status);
    ServiceResponse { status, body }
}

fn dataset_ingestion_status(body: &Value, success_status: u16) -> u16 {
    match body
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
    {
        Some("dataset_ingestion_not_found") => 404,
        Some("dataset_authority_mismatch") => 403,
        Some("dataset_ingestion_conflict" | "dataset_ingestion_canceled") => 409,
        Some("dataset_ingestion_persistence_failed" | "dataset_ingestion_evidence_failed") => 500,
        Some(_) => 400,
        None => success_status,
    }
}

fn nested_graphql_request(variables: &Value) -> Value {
    let mut request = variables
        .get("request")
        .cloned()
        .unwrap_or_else(|| variables.clone());
    let (Some(request_map), Some(variable_map)) = (request.as_object_mut(), variables.as_object())
    else {
        return request;
    };
    for key in [
        "controlPlaneCommandId",
        "controlPlaneCorrelationId",
        "controlPlaneSourceInterface",
        "controlPlaneSideEffectClass",
        "controlPlaneExplicitAuthority",
        "accountMode",
    ] {
        if let Some(value) = variable_map.get(key) {
            request_map.insert(key.to_string(), value.clone());
        }
    }
    if let Some(outer) = variable_map
        .get("authorityContext")
        .and_then(Value::as_object)
    {
        let mut authority = request_map
            .get("authorityContext")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        for key in ["surface", "accountMode"] {
            if let Some(value) = outer.get(key) {
                authority.insert(key.to_string(), value.clone());
            }
        }
        authority.entry("actor".to_string()).or_insert_with(|| {
            outer
                .get("actor")
                .cloned()
                .unwrap_or_else(|| json!("local-user"))
        });
        request_map.insert("authorityContext".to_string(), Value::Object(authority));
    }
    request
}

fn control_plane_needs_execution_context(path: &str) -> bool {
    path.contains("strategy-execution")
        || path.contains("run-center")
        || path.contains("scheduler")
        || path.contains("/orders")
        || path == "/run"
}

fn graphql_control_route(operation: &str) -> Option<&'static str> {
    match operation {
        "CreateDatasetIngestion" => Some("/dataset-ingestions"),
        "CancelDatasetIngestion" => Some("/dataset-ingestions/{ingestion_id}/cancel"),
        "InstallPluginManifest" => Some("/plugins/manifests"),
        "InstallPluginPackage" => Some("/plugins/packages"),
        "CreatePluginInstance" => Some("/plugins/instances"),
        "ConfigurePluginInstance" => Some("/plugins/instances/{instance_ref}/configuration"),
        "EnablePluginInstance" => Some("/plugins/instances/{instance_ref}:enable"),
        "DisablePluginInstance" => Some("/plugins/instances/{instance_ref}:disable"),
        "UpgradePluginInstance" => Some("/plugins/instances/{instance_ref}:upgrade"),
        "RollbackPluginInstance" => Some("/plugins/instances/{instance_ref}:rollback"),
        "RemovePluginInstance" => Some("/plugins/instances/{instance_ref}"),
        "StorePluginCredentials" => Some("/plugins/instances/{instance_ref}/credentials"),
        "StartPluginOAuth" => Some("/plugins/oauth/start"),
        "PluginOAuthStatus" => Some("/plugins/oauth/status"),
        "DisconnectPluginOAuth" => Some("/plugins/oauth/disconnect"),
        "RevokePluginCredentials" => Some("/plugins/instances/{instance_ref}/credentials"),
        "RefreshPluginHealth" => Some("/plugins/instances/{instance_ref}/health:refresh"),
        "RevokeEntitlementOverride" => Some("/entitlements/{grant_id}"),
        "PublishStrategy" => Some("/product/strategies/publish"),
        "SightlineSessionState" => Some("/product/sightline/session-state"),
        "PublishSightlineStrategyEditorSurface" => {
            Some("/product/sightline/strategy-editor/publish")
        }
        "PublishSightlineStudioSurface" => Some("/product/sightline/studio-surface/publish"),
        "SetSightlineSelection" => Some("/product/sightline/selections/set"),
        "ShareSightlineSelection" => Some("/product/sightline/selections/share"),
        "SightlineGetContext" => Some("/product/sightline/context/get"),
        "ConnectSightlineAgent" => Some("/product/sightline/agents/connect"),
        "SightlineUpdatePresence" => Some("/product/sightline/presence/update"),
        "SightlineCreateProposal" => Some("/product/sightline/proposals/create"),
        "SightlineRequestApproval" => Some("/product/sightline/approvals/request"),
        "SightlineNavigateSurface" => Some("/product/sightline/navigation/request"),
        "SightlineFocusNode" => Some("/product/sightline/nodes/focus"),
        "SightlineHighlightNode" => Some("/product/sightline/nodes/highlight"),
        "SightlineEvents" => Some("/product/sightline/events"),
        "SaveExecutionConfig" => Some("/product/strategy-execution-configs/save"),
        "ActivationReadiness" => Some("/product/strategy-execution-configs/activation-readiness"),
        "ActivateExecution" => Some("/product/strategy-execution-activations/activate"),
        "ControlStrategyExecution" => Some("/product/strategy-execution-activations/control"),
        "DeactivateStrategyExecution" => Some("/product/strategy-execution-activations/deactivate"),
        "RunStrategyOnce" => Some("/product/run-center/run-once"),
        "SchedulerStart" => Some("/product/scheduler/start"),
        "SchedulerRun" => Some("/scheduler/run"),
        "SchedulerStop" => Some("/product/scheduler/stop"),
        "ReconcileOrders" => Some("/orders/reconcile"),
        _ => None,
    }
}

fn mcp_control_route(name: &str) -> Option<&'static str> {
    match name {
        "studio.deployment.create" => Some("/studio/agent-deployments"),
        "studio.deployment.start" => Some("/studio/agent-deployments/{deployment_id}/start"),
        "studio.deployment.pause" => Some("/studio/agent-deployments/{deployment_id}/pause"),
        "studio.deployment.stop" => Some("/studio/agent-deployments/{deployment_id}/stop"),
        "studio.deployment.inspect" => Some("/studio/agent-deployments/{deployment_id}"),
        "studio.agent_run.inspect" => Some("/studio/agent-runs/{run_id}"),
        "studio.agent_run.events" => Some("/studio/agent-deployments/{deployment_id}/events"),
        "studio.agent_run.health" => Some("/studio/agent-runs/health"),
        "studio.agent_run.recover" => Some("/studio/agent-deployments/{deployment_id}/recover"),
        "tradeassembly.dataset_ingestion.create" => Some("/dataset-ingestions"),
        "tradeassembly.dataset_ingestion.cancel" => {
            Some("/dataset-ingestions/{ingestion_id}/cancel")
        }
        "tradeassembly.strategy.version.publish" => Some("/product/strategies/publish"),
        "tradeassembly.sightline.session_state" => Some("/product/sightline/session-state"),
        "tradeassembly.sightline.get_current_selection" => Some("/product/sightline/context/get"),
        "tradeassembly.sightline.get_context" => Some("/product/sightline/context/get"),
        "tradeassembly.sightline.create_proposal" => Some("/product/sightline/proposals/create"),
        "tradeassembly.sightline.request_approval" => Some("/product/sightline/approvals/request"),
        "tradeassembly.sightline.navigate_surface" => Some("/product/sightline/navigation/request"),
        "tradeassembly.sightline.list_events" => Some("/product/sightline/events"),
        "tradeassembly.execution.config.save" => Some("/product/strategy-execution-configs/save"),
        "tradeassembly.execution.readiness" => {
            Some("/product/strategy-execution-configs/activation-readiness")
        }
        "tradeassembly.execution.activate" => {
            Some("/product/strategy-execution-activations/activate")
        }
        "tradeassembly.execution.control" => {
            Some("/product/strategy-execution-activations/control")
        }
        "tradeassembly.execution.deactivate" => {
            Some("/product/strategy-execution-activations/deactivate")
        }
        "tradeassembly.execution.run" => Some("/product/run-center/run-once"),
        "tradeassembly.execution.scheduler.start" => Some("/product/scheduler/start"),
        "tradeassembly.execution.scheduler.run" => Some("/scheduler/run"),
        "tradeassembly.execution.scheduler.stop" => Some("/product/scheduler/stop"),
        _ => None,
    }
}

fn mcp_object_authorization_route(name: &str) -> Option<&'static str> {
    match name {
        "tradeassembly.strategy.get" => Some("/product/strategies/get"),
        "tradeassembly.strategy.save_draft" | "tradeassembly.strategy.draft.save" => {
            Some("/product/strategies/save-draft")
        }
        "tradeassembly.strategy.draft.select_node" => {
            Some("/product/strategies/semantic-selection")
        }
        "tradeassembly.strategy.draft.propose_patch" => {
            Some("/product/strategies/proposals/create")
        }
        "tradeassembly.strategy.draft.review_patch"
        | "tradeassembly.strategy.draft.apply_patch" => {
            Some("/product/strategies/proposals/review")
        }
        "tradeassembly.strategy.version.publish" => Some("/product/strategies/publish"),
        "tradeassembly.dataset_ingestion.get" => Some("/dataset-ingestions/{ingestion_id}"),
        "tradeassembly.dataset_ingestion.status" | "tradeassembly.dataset_ingestion.verify" => {
            Some("/dataset-ingestions/{ingestion_id}")
        }
        "tradeassembly.dataset_ingestion.cancel" => {
            Some("/dataset-ingestions/{ingestion_id}/cancel")
        }
        "tradeassembly.backtest.run" => Some("/backtests"),
        "tradeassembly.backtest.get"
        | "tradeassembly.backtest.report"
        | "tradeassembly.backtest.cancel"
        | "tradeassembly.backtest.retry"
        | "tradeassembly.backtest.replay"
        | "tradeassembly.backtest.export" => Some("/backtests/{run_id}"),
        "tradeassembly.robustness.run" | "tradeassembly.derivatives.create" => {
            Some("/backtests/{source_run_id}")
        }
        "tradeassembly.robustness.get"
        | "tradeassembly.robustness.process"
        | "tradeassembly.robustness.cancel"
        | "tradeassembly.robustness.retry"
        | "tradeassembly.robustness.replay"
        | "tradeassembly.robustness.export" => Some("/robustness-runs/{run_id}"),
        "tradeassembly.derivatives.get" | "tradeassembly.derivatives.export" => {
            Some("/derivatives-analyses/{analysis_id}")
        }
        "tradeassembly.comparison.create" => Some("/research-comparisons"),
        "tradeassembly.comparison.get" | "tradeassembly.comparison.export" => {
            Some("/research-comparisons/{comparison_id}")
        }
        "tradeassembly.credential.status" | "tradeassembly.credential.test" => {
            Some("/product/providers/credentials/test")
        }
        "tradeassembly.plugin.oauth.start"
        | "tradeassembly.plugin.oauth.status"
        | "tradeassembly.plugin.oauth.disconnect" => {
            Some("/plugins/instances/{instance_ref}/credentials")
        }
        "tradeassembly.execution.config.save" => Some("/product/strategy-execution-configs/save"),
        "tradeassembly.execution.readiness" => {
            Some("/product/strategy-execution-configs/activation-readiness")
        }
        "tradeassembly.execution.activate" => {
            Some("/product/strategy-execution-activations/activate")
        }
        "tradeassembly.execution.control" => {
            Some("/product/strategy-execution-activations/control")
        }
        "tradeassembly.execution.deactivate" => {
            Some("/product/strategy-execution-activations/deactivate")
        }
        _ => mcp_control_route(name),
    }
}

fn graphql_object_authorization_route(operation: &str) -> Option<&'static str> {
    match operation {
        "ProposeStrategyPatch" => Some("/product/strategies/proposals/create"),
        "ReviewStrategyPatch" | "ApplyStrategyPatch" => {
            Some("/product/strategies/proposals/review")
        }
        "StrategyDetail"
        | "StrategyInstrumentContext"
        | "StrategyVersionHistory"
        | "StrategyBuilder"
        | "StrategyContractMetadata"
        | "StrategyResearch"
        | "StrategyResearchWorkspace"
        | "StrategyExecution"
        | "StrategyExecutionWorkspace"
        | "SaveBuilderDraft"
        | "SelectStrategySemanticNode"
        | "ValidateBuilderDraft"
        | "PublishStrategy"
        | "DuplicateStrategy"
        | "ArchiveStrategy"
        | "RestoreStrategy"
        | "RunStrategyResearch"
        | "CreateResearchDataset"
        | "QueueStrategyResearchJob"
        | "StrategyResearchJobStatus"
        | "PromoteStrategyResearchToPaper"
        | "RunResearchSweep"
        | "CreateResearchUniverse" => Some("/product/strategies/get"),
        "CreateDatasetIngestion" => Some("/dataset-ingestions"),
        "DatasetIngestionGet"
        | "DatasetIngestionStatus"
        | "CancelDatasetIngestion"
        | "VerifyDatasetIngestion" => Some("/dataset-ingestions/{ingestion_id}"),
        "RunBacktest" => Some("/backtests"),
        "CreateResearchComparison" => Some("/research-comparisons"),
        "CreateRobustnessRun" | "CreateDerivativesAnalysis" => Some("/backtests/{source_run_id}"),
        "BacktestRun"
        | "BacktestReport"
        | "CancelBacktest"
        | "RetryBacktest"
        | "ReplayBacktest"
        | "BacktestReportExport" => Some("/backtests/{run_id}"),
        "ProcessBacktest" => Some("/backtests:process"),
        "RobustnessRun"
        | "RobustnessReport"
        | "CancelRobustnessRun"
        | "RetryRobustnessRun"
        | "ReplayRobustnessRun"
        | "ExportRobustnessRun" => Some("/robustness-runs/{run_id}"),
        "ProcessRobustnessRun" => Some("/robustness-runs:process"),
        "ObserveStudioSetup" => Some("/plugins/instances/{instance_ref}"),
        "DerivativesAnalysis" | "DerivativesAnalysisExport" => {
            Some("/derivatives-analyses/{analysis_id}")
        }
        "ResearchComparison" | "ResearchComparisonExport" => {
            Some("/research-comparisons/{comparison_id}")
        }
        "ResearchNotebookCreate"
        | "ResearchNotebookCompose"
        | "ResearchNotebookAttach"
        | "ResearchNotebookInspect"
        | "ResearchNotebookExport"
        | "ResearchNotebookReplay"
        | "AttributionJournalReview"
        | "AttributionJournalReport"
        | "AttributionJournalInspect"
        | "AttributionJournalReplay"
        | "AttributionJournalExport" => Some("/product/strategies/research"),
        _ => graphql_control_route(operation),
    }
}

fn enrich_sightline_control_plane_body(mut body: Value) -> Value {
    let Value::Object(map) = &mut body else {
        return body;
    };
    let refs = sightline_refs_from_map(map);
    if !refs.is_empty() && !map.contains_key("sightlineRefs") && !map.contains_key("sightline_refs")
    {
        map.insert("sightlineRefs".to_string(), json!(refs));
    }
    map.entry("client".to_string())
        .or_insert_with(|| json!("self"));
    map.entry("purpose".to_string())
        .or_insert_with(|| json!("agent_session_context"));
    if !map.contains_key("sessionId") && !map.contains_key("session_id") {
        map.insert("sessionId".to_string(), json!("room_tradeassembly_local"));
    }
    body
}

fn sightline_refs_from_map(map: &Map<String, Value>) -> Vec<String> {
    let mut refs = vec!["room_tradeassembly_local".to_string()];
    for key in [
        "roomId",
        "room_id",
        "surfaceId",
        "surface_id",
        "nodeId",
        "node_id",
        "nodeRef",
        "node_ref",
        "selectionId",
        "selection_id",
        "proposalId",
        "proposal_id",
        "approvalId",
        "approval_id",
        "commandId",
        "command_id",
    ] {
        if let Some(value) = map.get(key).and_then(Value::as_str) {
            refs.push(value.to_string());
        }
    }
    if let Some(actor_id) = map
        .get("actor")
        .and_then(Value::as_object)
        .and_then(|actor| {
            actor
                .get("actorId")
                .or_else(|| actor.get("actor_id"))
                .or_else(|| actor.get("id"))
        })
        .and_then(Value::as_str)
    {
        refs.push(format!("agent:{actor_id}"));
    }
    refs.sort();
    refs.dedup();
    refs
}

fn with_control_plane_metadata(
    mut body: Value,
    envelope: &control_plane::ControlPlaneCommandEnvelope,
) -> Value {
    let Value::Object(map) = &mut body else {
        return body;
    };
    let explicit_authority = map
        .get("authorityContext")
        .or_else(|| map.get("authority_context"))
        .is_some_and(Value::is_object);
    map.insert(
        "controlPlaneCommandId".to_string(),
        json!(envelope.command_id.clone()),
    );
    map.insert(
        "controlPlaneCorrelationId".to_string(),
        json!(envelope.effective_correlation_id()),
    );
    map.insert(
        "controlPlaneIdempotencyKey".to_string(),
        json!(envelope.idempotency_key.as_str()),
    );
    map.insert(
        "controlPlaneSourceInterface".to_string(),
        json!(envelope.source_interface.clone()),
    );
    map.insert(
        "controlPlaneSideEffectClass".to_string(),
        json!(envelope.side_effect_class.clone()),
    );
    map.insert(
        "controlPlaneExplicitAuthority".to_string(),
        json!(explicit_authority),
    );
    map.insert(
        "accountMode".to_string(),
        json!(envelope.authority.account_mode.clone()),
    );
    map.insert(
        "authorityContext".to_string(),
        json!({
            "actor": envelope.authority.actor.clone(),
            "surface": envelope.authority.surface.clone(),
            "accountMode": envelope.authority.account_mode.clone(),
        }),
    );
    body
}

fn inherit_trusted_correlation(
    mut envelope: control_plane::ControlPlaneCommandEnvelope,
    enriched_body: &Value,
) -> control_plane::ControlPlaneCommandEnvelope {
    if let Some(correlation_id) = enriched_body
        .get("controlPlaneCorrelationId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        envelope.correlation_id = correlation_id.to_string();
    }
    envelope
}

fn inherit_trusted_graphql_correlation(
    envelope: control_plane::ControlPlaneCommandEnvelope,
    enriched_request: &Value,
) -> control_plane::ControlPlaneCommandEnvelope {
    let variables = enriched_request.get("variables").unwrap_or(&Value::Null);
    let trusted_body = variables.get("request").unwrap_or(variables);
    inherit_trusted_correlation(envelope, trusted_body)
}

fn copy_execution_context(map: &mut Map<String, Value>, source: &Value) {
    for (target, source_keys) in [
        ("configId", &["configId", "id"][..]),
        ("strategyId", &["strategyId", "strategy_id"][..]),
        (
            "strategyVersionId",
            &["strategyVersionId", "version_id"][..],
        ),
        ("providerRef", &["providerRef", "provider_ref"][..]),
        ("mode", &["mode", "accountMode", "account_mode"][..]),
        ("accountMode", &["accountMode", "mode", "account_mode"][..]),
        ("symbol", &["symbol"][..]),
        ("timeframe", &["timeframe"][..]),
        (
            "controlPlaneCorrelationId",
            &["correlationId", "correlation_id"][..],
        ),
    ] {
        if map.contains_key(target) {
            continue;
        }
        if let Some(value) = source_keys
            .iter()
            .find_map(|key| source.get(*key).filter(|value| !value.is_null()).cloned())
        {
            map.insert(target.to_string(), value);
        }
    }
    if !map.contains_key("riskLimits") {
        if let Some(risk_limits) = source
            .get("riskLimits")
            .or_else(|| source.get("risk_limits"))
            .filter(|value| !value.is_null())
        {
            map.insert("riskLimits".to_string(), risk_limits.clone());
        }
    }
}

fn execution_activation_id_from_path(path: &str) -> Option<&str> {
    let suffix = "/ticks/evaluate";
    path.strip_prefix("/product/strategy-execution-activations/")?
        .strip_suffix(suffix)
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

fn plugin_instance_ref_from_path(path: &str) -> Option<&str> {
    path.strip_prefix("/plugins/instances/")?
        .split([':', '/'])
        .next()
        .filter(|value| !value.is_empty())
}

fn map_string(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| map.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn bind_authenticated_studio_request(
    mut request: Value,
    session: &auth::SessionValidationResult,
) -> Value {
    let Some(principal) = session.principal.as_ref().filter(|_| session.authenticated) else {
        return request;
    };
    let Some(variables) = request.get_mut("variables").and_then(Value::as_object_mut) else {
        return request;
    };
    variables.remove("_studioSession");
    bind_authenticated_studio_variables(variables, principal);
    if let Some(nested) = variables.get_mut("request").and_then(Value::as_object_mut) {
        bind_authenticated_studio_variables(nested, principal);
    }
    request
}

fn bind_authenticated_studio_variables(
    variables: &mut Map<String, Value>,
    principal: &auth::SessionPrincipal,
) {
    bind_authenticated_variables(variables, principal, "studio", "user", false, None);
}

fn bind_authenticated_invocation_value(
    mut value: Value,
    principal: &auth::SessionPrincipal,
    surface: &str,
    actor_kind: &str,
    preserve_requested_mcp_mode: bool,
    trusted_account_mode: Option<&str>,
) -> Value {
    let Some(map) = value.as_object_mut() else {
        return value;
    };
    bind_authenticated_variables(
        map,
        principal,
        surface,
        actor_kind,
        preserve_requested_mcp_mode,
        trusted_account_mode,
    );
    if let Some(nested) = map.get_mut("request").and_then(Value::as_object_mut) {
        bind_authenticated_variables(
            nested,
            principal,
            surface,
            actor_kind,
            preserve_requested_mcp_mode,
            trusted_account_mode,
        );
    }
    value
}

fn bind_authenticated_variables(
    variables: &mut Map<String, Value>,
    principal: &auth::SessionPrincipal,
    surface: &str,
    actor_kind: &str,
    preserve_requested_mcp_mode: bool,
    trusted_account_mode: Option<&str>,
) {
    let account_mode =
        if let Some(mode) = trusted_account_mode.filter(|mode| matches!(*mode, "paper" | "live")) {
            mode.to_string()
        } else if surface == "studio" || preserve_requested_mcp_mode {
            variables
                .get("mode")
                .or_else(|| variables.get("accountMode"))
                .or_else(|| variables.get("account_mode"))
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "paper" | "live"))
                .or_else(|| {
                    variables
                        .get("authorityContext")
                        .or_else(|| variables.get("authority_context"))
                        .and_then(|value| {
                            value
                                .get("accountMode")
                                .or_else(|| value.get("account_mode"))
                        })
                        .and_then(Value::as_str)
                        .filter(|value| matches!(*value, "paper" | "live"))
                })
                .unwrap_or("paper")
                .to_string()
        } else {
            "paper".to_string()
        };
    let actor = json!({
        "kind": actor_kind,
        "id": principal.subject,
        "principalUser": principal.subject,
    });
    let authority = json!({
        "actor": principal.subject,
        "principalUser": principal.subject,
        "surface": surface,
        "accountMode": account_mode,
    });
    variables.insert("actor".to_string(), actor);
    variables.insert("actorId".to_string(), json!(principal.subject));
    variables.insert("actor_id".to_string(), json!(principal.subject));
    variables.insert("actorKind".to_string(), json!(actor_kind));
    variables.insert("actor_kind".to_string(), json!(actor_kind));
    variables.insert("principalUser".to_string(), json!(principal.subject));
    variables.insert("principal_user".to_string(), json!(principal.subject));
    variables.insert("mode".to_string(), json!(account_mode));
    variables.insert("accountMode".to_string(), json!(account_mode));
    variables.insert("account_mode".to_string(), json!(account_mode));
    variables.insert("authorityContext".to_string(), authority.clone());
    variables.insert("authority_context".to_string(), authority);
    if surface == "mcp" {
        variables.insert(
            "callingAgent".to_string(),
            json!({"kind": "agent", "id": "tradeassembly.mcp_agent"}),
        );
    }
}

fn graphql_operation_name(request: &Value) -> String {
    if let Some(name) = request.get("operationName").and_then(Value::as_str) {
        if !name.is_empty() {
            return name.to_string();
        }
    }
    let query = request.get("query").and_then(Value::as_str).unwrap_or("");
    for keyword in ["query ", "mutation "] {
        if let Some(index) = query.find(keyword) {
            let rest = &query[index + keyword.len()..];
            return rest
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect();
        }
    }
    String::new()
}

fn graphql_field(operation: &str) -> &'static str {
    match operation {
        "LocalWorkspace" => "localWorkspace",
        "StrategyLibrary" => "strategyLibrary",
        "StrategyHome" => "strategyHome",
        "StrategyDetail" => "strategyDetail",
        "StrategyInstrumentContext" => "strategyInstrumentContext",
        "StrategyVersionHistory" => "strategyVersionHistory",
        "StrategyBuilder" => "strategyBuilder",
        "StrategyContractMetadata" => "strategyContractMetadata",
        "StrategyResearch" | "StrategyResearchWorkspace" => "strategyResearchWorkspace",
        "StrategyExecution" | "StrategyExecutionWorkspace" => "strategyExecutionWorkspace",
        "RunCenter" => "runCenter",
        "RobustnessRuns" => "robustnessRuns",
        "RobustnessRun" => "robustnessRun",
        "RobustnessReport" => "robustnessReport",
        "ObserveStudioSetup" => "observeStudioSetup",
        "DerivativesAnalyses" => "derivativesAnalyses",
        "DerivativesAnalysis" => "derivativesAnalysis",
        "DerivativesAnalysisExport" => "derivativesAnalysisExport",
        "ResearchComparisons" => "researchComparisons",
        "ResearchComparison" => "researchComparison",
        "ResearchComparisonExport" => "researchComparisonExport",
        "ResearchRollup" => "researchRollup",
        "DatasetIngestionList" => "datasetIngestionList",
        "DatasetIngestionGet" => "datasetIngestionGet",
        "DatasetIngestionStatus" => "datasetIngestionStatus",
        "AlertCenter" => "alertCenter",
        "DiscoverWorkspace" => "discoverWorkspace",
        "StrategyShareWorkspace" => "strategyShareWorkspace",
        "ConnectWorkspace" | "Providers" => "connectWorkspace",
        "PluginsProviders" => "pluginsProviders",
        "PluginManifests" => "pluginManifests",
        "PluginInstances" => "pluginInstances",
        "PluginInstance" => "pluginInstance",
        "Entitlements" => "entitlements",
        "ResolveCapability" => "pluginCapabilityResolution",
        "ResolveCapabilityGraph" => "capabilityGraphResolution",
        "SaveCapabilityGraphRevision" => "saveCapabilityGraphRevision",
        "CapabilityGraphRevision" => "capabilityGraphRevision",
        "CheckCapabilityGraphRevision" => "checkCapabilityGraphRevision",
        "AdminPluginManifestRegistry" => "adminPluginManifestRegistry",
        "StoreCredentials" => "storeCredentials",
        "TestCredentials" => "testCredentials",
        "InstallPluginManifest" => "installPluginManifest",
        "InstallPluginPackage" => "installPluginPackage",
        "CreatePluginInstance" => "createPluginInstance",
        "ConfigurePluginInstance" => "configurePluginInstance",
        "EnablePluginInstance" => "enablePluginInstance",
        "DisablePluginInstance" => "disablePluginInstance",
        "UpgradePluginInstance" => "upgradePluginInstance",
        "RollbackPluginInstance" => "rollbackPluginInstance",
        "RemovePluginInstance" => "removePluginInstance",
        "StorePluginCredentials" => "storePluginCredentials",
        "StartPluginOAuth" => "startPluginOAuth",
        "PluginOAuthStatus" => "pluginOAuthStatus",
        "DisconnectPluginOAuth" => "disconnectPluginOAuth",
        "RevokePluginCredentials" => "revokePluginCredentials",
        "RefreshPluginHealth" => "refreshPluginHealth",
        "RevokeEntitlementOverride" => "revokeEntitlementOverride",
        "CreateStrategy" => "createStrategy",
        "CreateStrategyAiDraft" => "createStrategyAiDraft",
        "SaveBuilderDraft" => "saveBuilderDraft",
        "SelectStrategySemanticNode" => "selectStrategySemanticNode",
        "ProposeStrategyPatch" => "proposeStrategyPatch",
        "ReviewStrategyPatch" => "reviewStrategyPatch",
        "ApplyStrategyPatch" => "applyStrategyPatch",
        "SightlineSessionState" => "sightlineSessionState",
        "PublishSightlineStrategyEditorSurface" => "publishSightlineStrategyEditorSurface",
        "PublishSightlineStudioSurface" => "publishSightlineStudioSurface",
        "SetSightlineSelection" => "setSightlineSelection",
        "ShareSightlineSelection" => "shareSightlineSelection",
        "SightlineGetContext" => "sightlineGetContext",
        "ConnectSightlineAgent" => "connectSightlineAgent",
        "SightlineUpdatePresence" => "sightlineUpdatePresence",
        "SightlineCreateProposal" => "sightlineCreateProposal",
        "SightlineRequestApproval" => "sightlineRequestApproval",
        "SightlineNavigateSurface" => "sightlineNavigateSurface",
        "SightlineFocusNode" => "sightlineFocusNode",
        "SightlineHighlightNode" => "sightlineHighlightNode",
        "SightlineEvents" => "sightlineEvents",
        "ValidateBuilderDraft" => "validateBuilderDraft",
        "ValidateStrategyExpression" => "validateStrategyExpression",
        "PublishStrategy" => "publishStrategy",
        "DuplicateStrategy" => "duplicateStrategy",
        "ArchiveStrategy" => "archiveStrategy",
        "RestoreStrategy" => "restoreStrategy",
        "CompileStrategyPreview" => "compileStrategyPreview",
        "RunStrategyResearch" => "runStrategyResearch",
        "CreateDatasetIngestion" => "createDatasetIngestion",
        "CancelDatasetIngestion" => "cancelDatasetIngestion",
        "VerifyDatasetIngestion" => "verifyDatasetIngestion",
        "RunScenarioValuation" => "runScenarioValuation",
        "RunMonteCarlo" => "runMonteCarlo",
        "RunPortfolioRisk" => "runPortfolioRisk",
        "PortfolioRiskReport" => "portfolioRiskReport",
        "FillQualityInspect" => "fillQualityInspect",
        "FillQualityReport" => "fillQualityReport",
        "FillQualityReplay" => "fillQualityReplay",
        "FillQualityExport" => "fillQualityExport",
        "AttributionJournalReview" => "attributionJournalReview",
        "AttributionJournalReport" => "attributionJournalReport",
        "AttributionJournalInspect" => "attributionJournalInspect",
        "AttributionJournalReplay" => "attributionJournalReplay",
        "AttributionJournalExport" => "attributionJournalExport",
        "LifecycleCalendarList" => "lifecycleCalendarList",
        "LifecycleCalendarInspect" => "lifecycleCalendarInspect",
        "LifecycleCalendarExport" => "lifecycleCalendarExport",
        "LifecycleCalendarReplay" => "lifecycleCalendarReplay",
        "ResearchNotebookCreate" => "researchNotebookCreate",
        "ResearchNotebookCompose" => "researchNotebookCompose",
        "ResearchNotebookAttach" => "researchNotebookAttach",
        "ResearchNotebookList" => "researchNotebookList",
        "ResearchNotebookInspect" => "researchNotebookInspect",
        "ResearchNotebookExport" => "researchNotebookExport",
        "ResearchNotebookReplay" => "researchNotebookReplay",
        "MonteCarloStatus" => "monteCarloStatus",
        "MonteCarloReport" => "monteCarloReport",
        "CreateResearchDataset" => "createResearchDataset",
        "QueueStrategyResearchJob" => "queueStrategyResearchJob",
        "StrategyResearchJobStatus" => "strategyResearchJobStatus",
        "PromoteStrategyResearchToPaper" => "promoteStrategyResearchToPaper",
        "SaveExecutionConfig" => "saveExecutionConfig",
        "ActivationReadiness" => "activationReadiness",
        "ActivateExecution" => "activateExecution",
        "ControlStrategyExecution" => "controlStrategyExecution",
        "DeactivateStrategyExecution" => "deactivateStrategyExecution",
        "UpdateProviderInstance" => "updateProviderInstance",
        "EnableProviderInstance" => "enableProviderInstance",
        "DisableProviderInstance" => "disableProviderInstance",
        "AcknowledgeAlert" => "acknowledgeAlert",
        "CreateStrategyShareSnapshot" => "createStrategyShareSnapshot",
        "RevokeStrategyShareSnapshot" => "revokeStrategyShareSnapshot",
        "RunBacktest" => "runBacktest",
        "CreateRobustnessRun" => "createRobustnessRun",
        "ProcessRobustnessRun" => "processRobustnessRun",
        "CancelRobustnessRun" => "cancelRobustnessRun",
        "RetryRobustnessRun" => "retryRobustnessRun",
        "ReplayRobustnessRun" => "replayRobustnessRun",
        "ExportRobustnessRun" => "exportRobustnessRun",
        "CreateDerivativesAnalysis" => "createDerivativesAnalysis",
        "CreateResearchComparison" => "createResearchComparison",
        "BacktestRuns" => "backtestRuns",
        "BacktestRun" => "backtestRun",
        "BacktestReport" => "backtestReport",
        "CancelBacktest" => "cancelBacktest",
        "RetryBacktest" => "retryBacktest",
        "ProcessBacktest" => "processBacktest",
        "ReplayBacktest" => "replayBacktest",
        "BacktestReportExport" => "backtestReportExport",
        "RunResearchSweep" => "runResearchSweep",
        "CreateResearchUniverse" => "createResearchUniverse",
        "RunStrategyOnce" => "runStrategyOnce",
        "SchedulerStart" => "schedulerStart",
        "SchedulerRun" => "schedulerRun",
        "SchedulerStop" => "schedulerStop",
        "ReconcileOrders" => "reconcileOrders",
        _ => "unknown",
    }
}

fn graphql_value_status(value: &Value) -> u16 {
    if value.get("errors").is_some() {
        return 400;
    }
    if value["error"]["retryable"].as_bool() == Some(true) {
        return 503;
    }
    if service_error_code(value).is_some() {
        let code = graphql_error_code(value);
        if code == "finance_authority_denied" || code == "authority_required" {
            403
        } else {
            400
        }
    } else {
        200
    }
}

fn graphql_error_code(value: &Value) -> String {
    service_error_code(value).unwrap_or_else(|| "graphql_operation_failed".to_string())
}

fn service_error_code(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| error.get("code"))
        .or_else(|| value.get("detail"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn graphql_duplicate_response(status: u16) -> Value {
    if status >= 400 {
        json!({"errors": [{"message": "idempotency_replay_failed"}]})
    } else {
        json!({"data": {"duplicate": true, "status": status}})
    }
}

fn graphql_duplicate_response_with_body(status: u16, mut body: Value) -> Value {
    if status >= 400 {
        return json!({"errors": [{"message": "idempotency_replay_failed"}]});
    }
    if let Some(data) = body.get_mut("data").and_then(Value::as_object_mut) {
        data.insert("_duplicate".to_string(), json!(true));
        return body;
    }
    json!({"data": {"duplicate": true, "status": status}})
}

fn strategy_id_from(value: &Value) -> String {
    value
        .get("strategyId")
        .or_else(|| value.get("strategy_id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("strat_local_btc_demo")
        .to_string()
}

fn provider_ref_from(value: &Value) -> String {
    value
        .get("providerRef")
        .or_else(|| value.get("provider_ref"))
        .or_else(|| value.get("pluginRef"))
        .or_else(|| value.get("plugin_ref"))
        .or_else(|| value.get("ref"))
        .and_then(Value::as_str)
        .unwrap_or("plugin-instance")
        .to_string()
}

fn explicit_provider_ref_from(value: &Value) -> Option<String> {
    value
        .get("providerRef")
        .or_else(|| value.get("provider_ref"))
        .or_else(|| value.get("pluginRef"))
        .or_else(|| value.get("plugin_ref"))
        .or_else(|| value.get("ref"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|provider_ref| !provider_ref.is_empty())
        .map(str::to_string)
}

pub(crate) fn portfolio_risk_body_from(value: &Value) -> Value {
    if value.get("portfolioRiskRequest").is_some()
        || value.get("concentrationThresholds").is_some()
        || value.get("concentration_thresholds").is_some()
    {
        return value.clone();
    }
    let strategy_id = strategy_id_from(value);
    let mut request = if value.is_object() {
        value.clone()
    } else {
        json!({})
    };
    let object = request.as_object_mut().expect("portfolio risk object");
    object
        .entry("strategyId".to_string())
        .or_insert_with(|| json!(strategy_id.clone()));
    object
        .entry("scopeKind".to_string())
        .or_insert_with(|| json!("strategy"));
    object
        .entry("concentrationThresholds".to_string())
        .or_insert_with(|| {
            json!([{
                "id": "strategy-max-notional",
                "metric": "notional",
                "groupBy": "sector",
                "maxNotional": 25.0,
                "severity": "warn",
                "userDefined": true
            }])
        });
    request
}

pub(crate) fn fill_quality_body_from(value: &Value) -> Value {
    let strategy_id = strategy_id_from(value);
    let analysis_id = value
        .get("analysisId")
        .or_else(|| value.get("analysis_id"))
        .and_then(Value::as_str)
        .unwrap_or("fill_quality_latest")
        .to_string();
    let mut request = if value.is_object() {
        value.clone()
    } else {
        json!({})
    };
    let object = request.as_object_mut().expect("fill quality object");
    object
        .entry("strategyId".to_string())
        .or_insert_with(|| json!(strategy_id));
    object
        .entry("analysisId".to_string())
        .or_insert_with(|| json!(analysis_id));
    object
        .entry("benchmark".to_string())
        .or_insert_with(|| json!("decision_quote_midpoint"));
    if !object.contains_key("executionEvents")
        && !object.contains_key("execution_events")
        && !object.contains_key("events")
    {
        object.insert(
            "executionEvents".to_string(),
            json!([
                demo_fill_quality_event(
                    "fq-fill-1",
                    &strategy_id,
                    "paper_fill",
                    2.0,
                    10.10,
                    0.02,
                    1_400,
                ),
                demo_fill_quality_event(
                    "fq-fill-2",
                    &strategy_id,
                    "partial_fill",
                    1.0,
                    10.20,
                    0.01,
                    1_900,
                )
            ]),
        );
    }
    request
}

pub(crate) fn attribution_journal_body_from(value: &Value) -> Value {
    let strategy_id = strategy_id_from(value);
    let review_id = value
        .get("reviewId")
        .or_else(|| value.get("review_id"))
        .and_then(Value::as_str)
        .unwrap_or("attribution_journal_latest")
        .to_string();
    let mut request = if value.is_object() {
        value.clone()
    } else {
        json!({})
    };
    let object = request.as_object_mut().expect("attribution journal object");
    object
        .entry("strategyId".to_string())
        .or_insert_with(|| json!(strategy_id.clone()));
    object
        .entry("reviewId".to_string())
        .or_insert_with(|| json!(review_id));
    object
        .entry("studioBaseUrl".to_string())
        .or_insert_with(|| json!("http://127.0.0.1:3001"));
    if !object.contains_key("executionEvents")
        && !object.contains_key("execution_events")
        && !object.contains_key("events")
    {
        object.insert(
            "executionEvents".to_string(),
            json!([
                {
                    "eventId": "exec-journal-win",
                    "refUri": "tradeassembly://execution/exec-journal-win",
                    "contentHash": "sha256:execjournalwin",
                    "underlying": "SPY",
                    "instrumentId": "occ:SPY260717C00550000",
                    "legId": "call-long",
                    "entryAtUnixMs": 1_784_800_000_000u64,
                    "exitAtUnixMs": 1_785_059_200_000u64,
                    "grossPnl": 120.0,
                    "fees": 1.3,
                    "netPnl": 118.7,
                    "quantity": 1.0,
                    "outcome": "win",
                    "fillQualityRef": "fill-quality-local"
                },
                {
                    "eventId": "exec-journal-loss",
                    "refUri": "tradeassembly://execution/exec-journal-loss",
                    "contentHash": "sha256:execjournalloss",
                    "underlying": "SPY",
                    "instrumentId": "occ:SPY260717P00530000",
                    "legId": "put-hedge",
                    "entryAtUnixMs": 1_784_886_400_000u64,
                    "exitAtUnixMs": 1_785_145_600_000u64,
                    "grossPnl": -32.0,
                    "fees": 1.1,
                    "netPnl": -33.1,
                    "quantity": 1.0,
                    "outcome": "loss",
                    "fillQualityRef": "fill-quality-local"
                }
            ]),
        );
    }
    if !object.contains_key("skipRejects") && !object.contains_key("skip_rejects") {
        object.insert(
            "skipRejects".to_string(),
            json!([
                {"reason": "outside_user_window", "count": 1, "sourceRef": "journal://skip/outside-user-window"},
                {"reason": "liquidity_filter", "count": 1, "sourceRef": "journal://skip/liquidity-filter"}
            ]),
        );
    }
    request
}

pub(crate) fn lifecycle_calendar_body_from(value: &Value) -> Value {
    let strategy_id = strategy_id_from(value);
    let timeline_id = value
        .get("timelineId")
        .or_else(|| value.get("timeline_id"))
        .and_then(Value::as_str)
        .unwrap_or("lifecycle_calendar_latest")
        .to_string();
    let mut request = if value.is_object() {
        value.clone()
    } else {
        json!({})
    };
    let object = request.as_object_mut().expect("lifecycle calendar object");
    object
        .entry("strategyId".to_string())
        .or_insert_with(|| json!(strategy_id));
    object
        .entry("timelineId".to_string())
        .or_insert_with(|| json!(timeline_id));
    object
        .entry("studioBaseUrl".to_string())
        .or_insert_with(|| json!("http://127.0.0.1:3001"));
    if !object.contains_key("ledgerState")
        && !object.contains_key("providerMetadata")
        && !object.contains_key("instrumentPacks")
        && !object.contains_key("userReminders")
    {
        object.insert("nowUnixMs".to_string(), json!(1_785_000_000_000u64));
        object.insert(
            "ledgerState".to_string(),
            json!({
                "positions": [{
                    "id": "position-spy-call",
                    "strategy_id": strategy_id,
                    "instrumentId": "occ:SPY260717C00550000",
                    "symbol": "SPY",
                    "assetClass": "option",
                    "accountScopeRef": "acct_scope_redacted",
                    "expirationUnixMs": 1_785_801_600_000u64
                }]
            }),
        );
        object.insert(
            "providerMetadata".to_string(),
            json!([{
                "providerRef": "local-derived",
                "earnings": [{
                    "instrumentId": "eq:SPY",
                    "symbol": "SPY",
                    "atUnixMs": 1_785_715_200_000u64,
                    "confidence": 0.82
                }],
                "dividends": [{
                    "instrumentId": "eq:SPY",
                    "symbol": "SPY",
                    "atUnixMs": 1_785_628_800_000u64,
                    "confidence": 0.78
                }]
            }]),
        );
        object.insert(
            "instrumentPacks".to_string(),
            json!([{
                "source": {"sourceRef": "local-pack", "providerRef": "local-derived"},
                "events": [{
                    "eventType": "last_trade_date",
                    "instrumentId": "occ:SPY260717C00550000",
                    "symbol": "SPY",
                    "positionId": "position-spy-call",
                    "accountScopeRef": "acct_scope_redacted",
                    "atUnixMs": 1_785_715_200_000u64,
                    "confidence": 0.95
                }]
            }]),
        );
        object.insert(
            "userReminders".to_string(),
            json!([{
                "id": "reminder-review-risk",
                "instrumentId": "occ:SPY260717C00550000",
                "symbol": "SPY",
                "atUnixMs": 1_785_196_800_000u64,
                "message": "Review position notes"
            }]),
        );
    }
    request
}

pub(crate) fn research_notebook_body_from(value: &Value) -> Value {
    let strategy_id = strategy_id_from(value);
    let mut request = if value.is_object() {
        value.clone()
    } else {
        json!({})
    };
    let object = request.as_object_mut().expect("research notebook object");
    object
        .entry("strategyId".to_string())
        .or_insert_with(|| json!(strategy_id));
    object
        .entry("studioBaseUrl".to_string())
        .or_insert_with(|| json!("http://127.0.0.1:3001"));
    request
}

fn demo_fill_quality_event(
    fill_id: &str,
    strategy_id: &str,
    event_kind: &str,
    quantity: f64,
    fill_price: f64,
    fee_amount: f64,
    filled_at_unix_ms: u64,
) -> Value {
    json!({
        "schemaVersion": "tradeassembly.execution_event.v1",
        "eventId": format!("event-{fill_id}"),
        "eventKind": event_kind,
        "mode": "paper",
        "orderIntent": {
            "orderIntentId": format!("intent-{fill_id}"),
            "strategyId": strategy_id,
            "instrumentId": "ul:equity:SPY",
            "side": "buy",
            "orderType": "limit",
            "quantity": quantity,
            "limitPrice": 10.25,
            "createdAtUnixMs": 1_000,
            "providerRef": "sim",
            "userActivationRef": "activation-user-approved",
            "accountScope": {
                "mode": "paper",
                "accountScopeRef": "acct_scope_redacted",
                "brokerAccountId": null,
                "rawAccountIdentifierExposed": false
            },
            "venueMetadata": {"venue": "paper"},
            "sourceProvenance": demo_fill_quality_provenance("order_intent")
        },
        "fill": {
            "fillId": fill_id,
            "status": if event_kind == "partial_fill" { "partially_filled" } else { "filled" },
            "filledQuantity": quantity,
            "fillPrice": fill_price,
            "filledAtUnixMs": filled_at_unix_ms,
            "feeAmount": fee_amount,
            "feeCurrency": "USD",
            "rejectReason": null,
            "cancelReason": null,
            "providerVenueMetadata": {"venue": "paper"},
            "sourceProvenance": demo_fill_quality_provenance("fill")
        },
        "decisionQuote": demo_fill_quality_quote("quote-decision", 9.95, 10.05, 1_000),
        "fillQuote": demo_fill_quality_quote("quote-fill", 10.00, 10.10, filled_at_unix_ms),
        "replayRef": {
            "replayRef": format!("replay://fills/{fill_id}"),
            "deterministic": true
        }
    })
}

fn demo_fill_quality_quote(ref_id: &str, bid: f64, ask: f64, captured_at_unix_ms: u64) -> Value {
    json!({
        "quoteRef": ref_id,
        "instrumentId": "ul:equity:SPY",
        "bid": bid,
        "ask": ask,
        "last": null,
        "capturedAtUnixMs": captured_at_unix_ms,
        "staleAfterUnixMs": captured_at_unix_ms + 1_000,
        "providerRef": "local-data",
        "venueMetadata": {"feed": "iex"},
        "sourceProvenance": demo_fill_quality_provenance(ref_id)
    })
}

fn demo_fill_quality_provenance(source_ref: &str) -> Value {
    json!({
        "source": "fixture",
        "sourceRef": source_ref,
        "collectedAtUnixMs": 1_000,
        "rawSecretsExposed": false
    })
}

fn mcp_submission_requested(arguments: &Value) -> bool {
    arguments["submit_orders"].as_bool().unwrap_or(false)
        || arguments["submit_exit"].as_bool().unwrap_or(false)
}

fn mcp_submission_has_explicit_authority(arguments: &Value) -> bool {
    arguments["allow_mcp_order_submission"]
        .as_bool()
        .unwrap_or(false)
        && arguments["authority_context"].is_object()
        && arguments["idempotency_key"].as_str().is_some()
        && matches!(
            arguments["account_mode"].as_str(),
            Some("paper") | Some("live")
        )
}

fn mcp_duplicate_response(name: &str, status: u16) -> Value {
    if status >= 400 {
        mcp::tool_error(
            name,
            "idempotency_replay_failed",
            "MCP command idempotency key replays a prior failed command.",
            Some(json!({"status": status})),
        )
    } else {
        mcp::call_tool_with_payload(
            name,
            json!({"ok": true, "duplicate": true, "status": status}),
        )
    }
}

fn mcp_duplicate_response_with_body(name: &str, status: u16, mut body: Value) -> Value {
    if status >= 400 {
        return mcp::tool_error(
            name,
            "idempotency_replay_failed",
            "MCP command idempotency key replays a prior failed command.",
            Some(json!({"status": status})),
        );
    }
    if let Some(object) = body.as_object_mut() {
        object.insert("duplicate".to_string(), json!(true));
    }
    body
}

fn mcp_execution_run_body(arguments: &Value) -> Value {
    let mut body = arguments.as_object().cloned().unwrap_or_default();
    if !body.contains_key("activationId") {
        body.insert(
            "activationId".to_string(),
            arguments
                .get("activation_id")
                .cloned()
                .unwrap_or_else(|| json!("activation-btc-exit-demo")),
        );
    }
    if !body.contains_key("submitExit") {
        body.insert(
            "submitExit".to_string(),
            json!(arguments["submit_exit"].as_bool().unwrap_or(false)),
        );
    }
    if !body.contains_key("submitOrders") {
        body.insert(
            "submitOrders".to_string(),
            json!(arguments["submit_orders"].as_bool().unwrap_or(false)),
        );
    }
    if !body.contains_key("idempotencyKey") {
        if let Some(value) = arguments.get("idempotency_key") {
            body.insert("idempotencyKey".to_string(), value.clone());
        }
    }
    if !body.contains_key("accountMode") {
        if let Some(value) = arguments.get("account_mode") {
            body.insert("accountMode".to_string(), value.clone());
        }
    }
    if !body.contains_key("authorityContext") {
        if let Some(value) = arguments.get("authority_context") {
            body.insert("authorityContext".to_string(), value.clone());
        }
    }
    Value::Object(body)
}

fn path_strategy_id(path: &str) -> &str {
    path.trim_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or("strat_local_btc_demo")
}

fn provider_from_path(path: &str) -> &str {
    path.trim_matches('/')
        .split('/')
        .nth(1)
        .unwrap_or("plugin-instance")
}

fn plugin_path_ref(path: &str) -> &str {
    path.trim_matches('/')
        .split('/')
        .nth(2)
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
}

fn path_id(path: &str) -> &str {
    path.trim_matches('/').split('/').nth(1).unwrap_or("local")
}

fn sample_order() -> Value {
    json!({"id": "order_local_0001", "order_id": "order_local_0001", "status": "filled", "symbol": "BTC/USD", "side": "buy", "qty": 0.0002})
}

fn sample_position() -> Value {
    json!({"id": "position-local-btc", "strategy_id": "strat_local_btc_demo", "symbol": "BTC/USD", "status": "open", "qty": 0.0002})
}

pub(crate) fn journal_events(service: &TradeAssemblyService) -> Value {
    match journal_view::events(service) {
        Ok(events) => json!(events),
        Err(response) => response.body,
    }
}

fn evidence(event_type: &str) -> Value {
    json!({"event_type": event_type, "journaled": true, "runtime": "rust"})
}

pub(crate) fn workspace_evidence(service: &TradeAssemblyService) -> Value {
    match journal_view::events(service) {
        Ok(events) => json!({
            "journalEvents": events.len(),
            "runs": Value::Null,
            "orders": Value::Null,
            "backtests": Value::Null,
            "latestRunId": Value::Null,
            "latestOrderStatus": Value::Null,
            "latestBacktestStatus": Value::Null,
        }),
        Err(response) => response.body,
    }
}

fn strategy_lifecycle() -> Value {
    json!({"total": 2, "active": 1, "draft": 1, "paused": 0, "versions": 2})
}

fn performance_summary() -> Value {
    json!({"total_orders": 1, "submitted_orders": 1, "filled_orders": 1, "submit_attempts": 1, "active_positions": 1, "realized_pnl": 0})
}

pub(crate) fn execution_body(service: &TradeAssemblyService) -> Value {
    json!({
        "activation": {"status": "idle"},
        "activationHistory": [],
        "journal": {"rows": journal_events(service), "totals": {}},
        "positions": {"items": [sample_position()], "summary": {"openPositions": 1}},
        "positionLifecycle": {"summary": {"status": "open"}, "timeline": []},
        "accountImpact": {"openPositions": 1, "grossExposure": 8.6, "netExposure": 8.6},
        "divergence": {"items": []},
        "changes": {"items": []},
        "performance": {"points": []},
        "stats": {"closedTrades": 0, "openTrades": 1},
        "runs": {"attempts": [{"id": "attempt-local"}], "brokerEvidence": [sample_order()]},
        "replay": {"counts": {"broker.order_submitted": 1}},
    })
}

#[allow(dead_code)]
fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(Map::from_iter(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    ))
}

#[cfg(test)]
mod finance_authority_diagnostic_tests {
    use super::finance_authority_denied_response;

    #[test]
    fn retains_known_warden_400_diagnostic() {
        let response = finance_authority_denied_response(
            "finance_authority:warden_request_failed:400:invalid_request:ignored",
        );
        assert_eq!(response.status, 403);
        assert_eq!(
            response.body["error"]["details"]["reason"],
            "warden_request_failed"
        );
        assert_eq!(response.body["error"]["details"]["httpStatus"], 400);
        assert_eq!(
            response.body["error"]["details"]["wardenCode"],
            "invalid_request"
        );
    }

    #[test]
    fn retains_extension_identity_mismatch_reason() {
        let response = finance_authority_denied_response(
            "finance_authority:extension_identity_mismatch:secret-token",
        );
        assert_eq!(response.status, 403);
        assert_eq!(
            response.body["error"]["details"]["reason"],
            "extension_identity_mismatch"
        );
    }

    #[test]
    fn excludes_unknown_reason_code_and_trailing_text() {
        let response = finance_authority_denied_response(
            "finance_authority:remote_failure:499:unknown_code:secret-token-fragment",
        );
        let rendered = response.body.to_string();
        assert_eq!(response.status, 403);
        assert_eq!(response.body["error"]["details"], serde_json::json!({}));
        assert!(!rendered.contains("remote_failure"));
        assert!(!rendered.contains("unknown_code"));
        assert!(!rendered.contains("secret-token-fragment"));
    }
}

#[cfg(test)]
mod local_setup_identity_tests {
    use super::*;

    #[test]
    fn local_setup_identity_is_bound_as_an_application() {
        let principal = auth::SessionPrincipal {
            provider: "tradeassembly_local_setup".to_string(),
            issuer: "tradeassembly://local-setup".to_string(),
            subject: "tradeassembly.local_setup".to_string(),
            email: None,
            email_verified: None,
            name: Some("TradeAssembly local setup".to_string()),
            picture: None,
        };

        let bound = bind_authenticated_invocation_value(
            json!({"actor": {"kind": "user", "id": "forged"}}),
            &principal,
            "local_setup",
            "application",
            false,
            None,
        );

        assert_eq!(bound["actor"]["kind"], "application");
        assert_eq!(bound["actor"]["id"], "tradeassembly.local_setup");
        assert_eq!(bound["actorKind"], "application");
        assert_eq!(bound["principalUser"], "tradeassembly.local_setup");
        assert_eq!(bound["authorityContext"]["surface"], "local_setup");
    }
}

#[cfg(test)]
mod test_database_factory_safety_tests {
    use super::assert_test_database_is_owned;
    use std::fs;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    #[test]
    fn fresh_fixture_can_reopen_but_unregistered_populated_file_is_untouched() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("fixture.db");
        assert_test_database_is_owned(path.to_str().expect("utf8 path"));

        let sentinel = b"customer-state-sentinel";
        fs::write(&path, sentinel).expect("write sentinel");
        assert_test_database_is_owned(path.to_str().expect("utf8 path"));
        assert_eq!(fs::read(&path).expect("read sentinel"), sentinel);

        let foreign = directory.path().join("foreign.db");
        fs::write(&foreign, sentinel).expect("write foreign sentinel");
        let rejected = catch_unwind(AssertUnwindSafe(|| {
            assert_test_database_is_owned(foreign.to_str().expect("utf8 path"));
        }));
        assert!(rejected.is_err());
        assert_eq!(fs::read(&foreign).expect("read foreign sentinel"), sentinel);
        // A rejected attempt must not poison every subsequent fixture request.
        assert_test_database_is_owned(path.to_str().expect("utf8 path"));
        let next = directory.path().join("next.db");
        assert_test_database_is_owned(next.to_str().expect("utf8 path"));
    }

    #[cfg(unix)]
    #[test]
    fn replaced_file_and_symlink_alias_do_not_gain_fixture_authority() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("fixture.db");
        assert_test_database_is_owned(path.to_str().expect("utf8 path"));
        fs::write(&path, b"registered").expect("write fixture");

        let replacement = directory.path().join("replacement.db");
        fs::write(&replacement, b"replacement").expect("write replacement");
        fs::rename(&replacement, &path).expect("replace fixture");
        let replaced = catch_unwind(AssertUnwindSafe(|| {
            assert_test_database_is_owned(path.to_str().expect("utf8 path"));
        }));
        assert!(replaced.is_err());

        let target = directory.path().join("customer.db");
        fs::write(&target, b"customer sentinel").expect("write customer sentinel");
        let alias = directory.path().join("fixture-alias.db");
        std::os::unix::fs::symlink(&target, &alias).expect("create alias");
        let aliased = catch_unwind(AssertUnwindSafe(|| {
            assert_test_database_is_owned(alias.to_str().expect("utf8 path"));
        }));
        assert!(aliased.is_err());
        assert_eq!(
            fs::read(&target).expect("read customer sentinel"),
            b"customer sentinel"
        );
    }
}

#[cfg(test)]
mod snapshot_identity_tests {
    use super::*;

    #[test]
    fn snapshot_reads_never_substitute_a_fixed_demo_record() {
        let service = TradeAssemblyService::test_local(":memory:");
        for path in [
            "/product/share-snapshots/view",
            "/product/share-snapshots/export",
        ] {
            for id in ["missing", "latest", "snapshot_local"] {
                let response = service.handle_http("POST", path, json!({"snapshotId":id}));
                assert_eq!(response.status, 404, "{:?}", response.body);
                assert_eq!(response.body["error"]["code"], "snapshot_unavailable");
                assert!(response.body.get("factsheet").is_none());
            }
        }
    }
}
