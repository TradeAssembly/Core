//! Browser companion for the local MCP. No Studio process or source files required.
//! The loopback capability is process-local; durable records contain no tokens.

use super::{authenticated_service, register_verified_identity};
use crate::{
    cli_identity::CliIdentityManager,
    identity_bootstrap::IdentityBootstrap,
    ports::{AuthorityContext, IdempotencyKey, SideEffectContext},
    service::TradeAssemblyService,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

const NAMESPACE: &str = "browser_onboarding_v1";
const LIFETIME_MS: i64 = 30 * 60 * 1000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Attempt {
    id: String,
    mode: String,
    environment: String,
    instance_ref: Option<String>,
    actor: Option<String>,
    connection_id: Option<String>,
    authorization_url: Option<String>,
    #[serde(default)]
    relay_requested: bool,
    #[serde(default)]
    last_error: Option<String>,
    expires_at_ms: i64,
    cancelled: bool,
    #[serde(default)]
    api_keys_stored: bool,
    #[serde(default)]
    api_keys_requested: bool,
    #[serde(default)]
    api_key_intent: Option<ApiKeyIntent>,
    #[serde(default)]
    saved_paper_token_requested: bool,
    #[serde(default)]
    saved_paper_token_stored: bool,
    #[serde(default)]
    saved_paper_token_intent: Option<ApiKeyIntent>,
}

/// Binds the operation, never the submitted credential values.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApiKeyIntent {
    package_sha256: String,
    prior_revision_ref: String,
    #[serde(default)]
    enable_sequence: u64,
}

#[derive(Clone)]
pub(super) struct BrowserOnboarding {
    service: TradeAssemblyService,
    identity: CliIdentityManager,
    local_identity: CliIdentityManager,
    profile: Option<crate::connection_profile::ConnectionProfile>,
    profile_error: Option<String>,
    bootstrap: IdentityBootstrap,
    listener: Arc<Mutex<Option<String>>>,
    operation: Arc<Mutex<()>>,
}

impl BrowserOnboarding {
    pub(super) async fn login(&self, start: bool) -> Result<Value, String> {
        if let Some(error) = self.profile_error.as_ref() {
            return Err(error.clone());
        }
        if start {
            self.bootstrap.start().await
        } else {
            self.bootstrap.status().await
        }
    }
    pub(super) fn new(service: &TradeAssemblyService, identity: &CliIdentityManager) -> Self {
        let path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("connection-profile.json")));
        let loaded = path
            .filter(|p| p.exists())
            .map(|p| crate::connection_profile::ConnectionProfile::read(&p));
        let (profile, profile_error) = match loaded {
            Some(Ok(p)) => (Some(p), None),
            Some(Err(e)) => (None, Some(e)),
            None => (None, None),
        };
        let hosted = profile
            .as_ref()
            .map(|p| CliIdentityManager::new(p.configure(identity.configuration())))
            .unwrap_or_else(|| identity.clone());
        Self {
            service: service.clone(),
            identity: hosted.clone(),
            local_identity: identity.clone(),
            profile,
            profile_error,
            bootstrap: IdentityBootstrap::new(hosted.configuration().clone()),
            listener: Arc::new(Mutex::new(None)),
            operation: Arc::new(Mutex::new(())),
        }
    }

    pub(super) fn call(&self, name: &str, args: Value) -> Value {
        let reference = args["onboardingId"].as_str().map(str::to_owned);
        match self.handle(name, args) {
            Ok(value) => value,
            Err(code) => {
                let code = if code.len() <= 128
                    && code
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                {
                    code
                } else {
                    "onboarding_operation_failed".into()
                };
                let browser_url = reference
                    .as_ref()
                    .filter(|id| self.read(id).ok().flatten().is_some())
                    .and_then(|id| {
                        self.ensure_listener()
                            .ok()
                            .map(|base| format!("{base}/{id}"))
                    });
                json!({"ready":false,"status":"failed","error":{"code":code,"message":error_message(&code)},
                    "onboardingId":reference,"browserUrl":browser_url,
                    "nextAction":"Use the connection browser to retry. Do not paste credentials or edit configuration."})
            }
        }
    }

    fn handle(&self, name: &str, args: Value) -> Result<Value, String> {
        let _guard = self.operation.lock().map_err(|_| "onboarding_busy")?;
        let _process_guard = self.process_lock()?;
        if let Some(error) = self.profile_error.as_ref() {
            return Err(error.clone());
        }
        if name == "tradeassembly.onboarding.start" || name == "tradeassembly.broker.connect" {
            let mode = args["mode"]
                .as_str()
                .filter(|v| matches!(*v, "paper" | "live"))
                .ok_or("onboarding_mode_required")?;
            let environment = args["environment"]
                .as_str()
                .or_else(|| self.profile.as_ref().map(|p| p.environment.as_str()))
                .filter(|v| matches!(*v, "staging" | "production"))
                .ok_or("onboarding_environment_required")?;
            if self
                .profile
                .as_ref()
                .is_some_and(|p| p.environment != environment)
            {
                return Err("onboarding_environment_mismatch".into());
            }
            let key = args["idempotency_key"]
                .as_str()
                .filter(|v| !v.is_empty() && v.len() <= 256)
                .ok_or("onboarding_idempotency_required")?;
            let id = format!("{:x}", Sha256::digest(key.as_bytes()));
            let requested_instance = args["instanceRef"].as_str().map(str::to_owned);
            let attempt = match self.read(&id)? {
                Some(existing) => {
                    if existing.mode != mode
                        || existing.environment != environment
                        || existing.relay_requested != args["relay"].as_bool().unwrap_or(false)
                        || requested_instance
                            .as_ref()
                            .is_some_and(|v| existing.instance_ref.as_ref() != Some(v))
                    {
                        return Err("onboarding_idempotency_conflict".into());
                    }
                    existing
                }
                None => {
                    let attempt = Attempt {
                        id,
                        mode: mode.into(),
                        environment: environment.into(),
                        instance_ref: requested_instance,
                        actor: None,
                        connection_id: None,
                        authorization_url: None,
                        relay_requested: args["relay"].as_bool().unwrap_or(false),
                        last_error: None,
                        expires_at_ms: now_ms() + LIFETIME_MS,
                        cancelled: false,
                        api_keys_stored: false,
                        api_keys_requested: false,
                        api_key_intent: None,
                        saved_paper_token_requested: false,
                        saved_paper_token_stored: false,
                        saved_paper_token_intent: None,
                    };
                    self.save(&attempt)?;
                    attempt
                }
            };
            return self.public_status(&attempt);
        }
        if !matches!(
            name,
            "tradeassembly.onboarding.status" | "tradeassembly.onboarding.cancel"
        ) {
            return Err("onboarding_operation_invalid".into());
        }
        let id = args["onboardingId"]
            .as_str()
            .ok_or("onboarding_reference_required")?;
        let mut attempt = self.read(id)?.ok_or("onboarding_not_found")?;
        if name == "tradeassembly.onboarding.cancel" {
            self.cancel(&mut attempt)?;
        }
        self.public_status(&attempt)
    }

    fn cancel(&self, attempt: &mut Attempt) -> Result<(), String> {
        if let (Some(instance), Some(connection)) = (&attempt.instance_ref, &attempt.connection_id)
        {
            let owner = tokio::runtime::Handle::current()
                .block_on(self.local_identity.current_identity())?;
            register_verified_identity(&self.service, &owner)?;
            authenticated_service(&self.service, &owner)
                .cancel_broker_connection_attempt(instance, connection)?;
        }
        attempt.cancelled = true;
        attempt.authorization_url = None;
        self.save(attempt)
    }

    fn read(&self, id: &str) -> Result<Option<Attempt>, String> {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("onboarding_reference_invalid".into());
        }
        self.service
            .runtime()
            .storage
            .get_json(NAMESPACE, id)?
            .map(|v| serde_json::from_value(v).map_err(|_| "onboarding_state_invalid".into()))
            .transpose()
    }

    fn process_lock(&self) -> Result<std::fs::File, String> {
        let path = format!("{}.onboarding.lock", self.service.db());
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|_| "onboarding_state_unavailable")?;
        file.try_lock().map_err(|_| "onboarding_busy")?;
        Ok(file)
    }

    fn save(&self, attempt: &Attempt) -> Result<(), String> {
        let context = SideEffectContext::new(
            AuthorityContext {
                actor: "local-installation".into(),
                surface: "browser_onboarding".into(),
                account_mode: attempt.mode.clone(),
            },
            IdempotencyKey::new(format!(
                "onboarding:{}:{}",
                attempt.id,
                rand::random::<u128>()
            ))?,
        );
        self.service.runtime().storage.put_json(
            NAMESPACE,
            &attempt.id,
            serde_json::to_value(attempt).map_err(|_| "onboarding_state_invalid")?,
            &context,
        )
    }

    fn ensure_listener(&self) -> Result<String, String> {
        let mut active = self.listener.lock().map_err(|_| "onboarding_busy")?;
        if let Some(base) = active.as_ref() {
            return Ok(base.clone());
        }
        let socket = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|_| "onboarding_listener_unavailable")?;
        socket
            .set_nonblocking(true)
            .map_err(|_| "onboarding_listener_unavailable")?;
        let address = socket
            .local_addr()
            .map_err(|_| "onboarding_listener_unavailable")?;
        let capability = format!("{:032x}", rand::random::<u128>());
        let prefix = format!("/{capability}");
        let base = format!("http://{address}{prefix}");
        let state = self.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("tradeassembly-onboarding".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Runtime::new() else {
                    return;
                };
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(socket) else {
                        return;
                    };
                    let router = Router::new()
                        .route(&format!("{prefix}/{{id}}"), get(page))
                        .route(&format!("{prefix}/{{id}}/connection"), get(connection))
                        .route(&format!("{prefix}/{{id}}/{{action}}"), post(action))
                        .with_state(state);
                    let _ = ready_tx.send(());
                    let _ = axum::serve(listener, router).await;
                });
            })
            .map_err(|_| "onboarding_listener_unavailable")?;
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|_| "onboarding_listener_unavailable")?;
        *active = Some(base.clone());
        Ok(base)
    }

    fn public_status(&self, attempt: &Attempt) -> Result<Value, String> {
        let status = if attempt.cancelled {
            "cancelled"
        } else if now_ms() >= attempt.expires_at_ms {
            "expired"
        } else {
            "browser_action_required"
        };
        let mut result = json!({"onboardingId":attempt.id,"status":status,"ready":false,
            "instanceRef":attempt.instance_ref,"connectionId":attempt.connection_id,
            "browserUrl":format!("{}/{}",self.ensure_listener()?,attempt.id),
            "mode":attempt.mode,"environment":attempt.environment,
            "expiresAtMs":attempt.expires_at_ms,"nextAction":"Open browserUrl to sign in and connect your broker."});
        if let Some(code) = attempt.last_error.as_ref() {
            result["lastError"] = json!({"code":code,"message":error_message(code)});
        }
        if status != "browser_action_required" {
            result["nextAction"] = json!(
                "This attempt has ended. Call tradeassembly.onboarding.start with a new idempotency_key and the user's chosen settings. Do not reopen the old authorization URL."
            );
            return Ok(result);
        }
        if attempt.api_keys_requested || attempt.saved_paper_token_requested {
            let mut resumed = attempt.clone();
            let service = self.bound_local_service(&resumed)?;
            let _oauth_guard = if attempt.saved_paper_token_requested {
                Some(
                    service.saved_paper_token_lock(
                        attempt
                            .instance_ref
                            .as_deref()
                            .ok_or("onboarding_broker_prepare_required")?,
                    )?,
                )
            } else {
                None
            };
            self.reconcile_local_credentials(
                &service,
                &mut resumed,
                attempt.saved_paper_token_requested,
            )?;
            let attempt = &resumed;
            let method = if attempt.saved_paper_token_requested {
                "saved-oauth-grant"
            } else {
                "api-key"
            };
            result["hubAuthenticated"] = json!(false);
            result["credentialMethod"] = json!(method);
            result["relayRequested"] = json!(attempt.relay_requested);
            result["relay"] = json!({"status":if attempt.relay_requested {"unchecked"} else {"not_requested"},"permitted":false});
            if !attempt.api_keys_stored && !attempt.saved_paper_token_stored {
                return Ok(result);
            }
            let instance = attempt
                .instance_ref
                .as_deref()
                .ok_or("onboarding_broker_prepare_failed")?;
            let verification = tool_payload(
                &service,
                "tradeassembly.broker.verify",
                verification_arguments(&attempt.id, instance, &attempt.mode),
            )?;
            let brokers = tool_payload(
                &service,
                "tradeassembly.broker.status",
                json!({"instanceRef":instance}),
            )?;
            let verified = brokers["instances"][0]["accountVerified"] == true;
            result["brokerConnected"] = json!(verified);
            result["verification"] = verification;
            result["credentialMethod"] = json!(method);
            let warden_config = self.local_identity.configuration().clone();
            let warden_ready =
                std::thread::spawn(move || super::preflight_api_warden(&warden_config))
                    .join()
                    .is_ok_and(|result| result.is_ok());
            result["wardenReady"] = json!(warden_ready);
            if verified && warden_ready && !attempt.relay_requested {
                result["status"] = json!("ready");
                result["ready"] = json!(true);
                result["nextAction"] =
                    json!("Connection verified. Strategy activation is a separate user decision.");
            } else if verified && warden_ready && attempt.relay_requested {
                result["status"] = json!("entitlement_required");
                result["nextAction"] = json!("Paper broker connection verified. Complete the separate Relay sign-in and subscription check.");
            }
            return Ok(result);
        }
        let auth = tokio::runtime::Handle::current().block_on(self.bootstrap.status())?;
        result["identity"] = auth.clone();
        result["hubAuthenticated"] = json!(false);
        if auth["authenticated"] != true {
            return Ok(result);
        }
        let (service, identity) = self.bound_service(attempt)?;
        let token = tokio::runtime::Handle::current().block_on(
            crate::workos_identity::WorkosAuthManager::new(self.identity.configuration().clone())
                .access_token_for_actor(&identity.stable_identity_id),
        )?;
        tokio::runtime::Handle::current().block_on(crate::hub_identity::project_session(
            self.identity.configuration(),
            &token,
            &identity.subject,
        ))?;
        result["hubAuthenticated"] = json!(true);
        result["relayRequested"] = json!(attempt.relay_requested);
        let relay = if attempt.relay_requested {
            self.check_entitlement(&token)?
        } else {
            json!({"status":"not_requested","permitted":false})
        };
        result["relay"] = relay.clone();
        if let (Some(connection), Some(instance)) = (&attempt.connection_id, &attempt.instance_ref)
        {
            let oauth = tool_payload(
                &service,
                "tradeassembly.plugin.oauth.status",
                json!({"instanceRef":instance,"connectionId":connection}),
            )?;
            result["brokerAuthorization"] =
                json!({"status":oauth["status"],"ackPending":oauth["ackPending"]});
            if oauth["status"] == "ready" {
                let verification = tool_payload(
                    &service,
                    "tradeassembly.broker.verify",
                    verification_arguments(&attempt.id, instance, &attempt.mode),
                )?;
                let brokers = tool_payload(
                    &service,
                    "tradeassembly.broker.status",
                    json!({"instanceRef":instance}),
                )?;
                let verified = brokers["instances"][0]["accountVerified"] == true;
                result["brokerConnected"] = json!(verified);
                result["verification"] = verification;
                result["accountRef"] = oauth["accountId"].clone();
                let warden_config = self.local_identity.configuration().clone();
                let warden_ready =
                    std::thread::spawn(move || super::preflight_api_warden(&warden_config))
                        .join()
                        .is_ok_and(|result| result.is_ok());
                result["wardenReady"] = json!(warden_ready);
                // Full readiness additionally requires an entitlement for requested hosted services.
                if verified
                    && warden_ready
                    && oauth["ackPending"] != true
                    && (!attempt.relay_requested || relay["permitted"] == true)
                {
                    result["status"] = json!("ready");
                    result["ready"] = json!(true);
                    result["nextAction"] = json!(
                        "Connection verified. Strategy activation is a separate user decision."
                    );
                } else if verified && attempt.relay_requested && relay["permitted"] != true {
                    result["status"] = json!("entitlement_required");
                } else if !warden_ready {
                    result["status"] = json!("warden_unavailable");
                }
            }
        }
        Ok(result)
    }

    fn bound_service(
        &self,
        attempt: &Attempt,
    ) -> Result<(TradeAssemblyService, crate::cli_identity::CliIdentity), String> {
        let handle = tokio::runtime::Handle::current();
        let identity = handle.block_on(self.identity.current_identity())?;
        if identity.issuer == "local-owner" {
            return Err("hosted_sign_in_required".into());
        }
        if attempt
            .actor
            .as_ref()
            .is_some_and(|a| a != &identity.stable_identity_id)
        {
            return Err("onboarding_identity_changed".into());
        }
        let owner = handle.block_on(self.local_identity.current_identity())?;
        register_verified_identity(&self.service, &owner)?;
        let service = authenticated_service(&self.service, &owner).with_hosted_connection(
            self.identity.configuration().clone(),
            identity.stable_identity_id.clone(),
            self.profile.clone(),
        );
        Ok((service, identity))
    }

    fn bound_local_service(&self, attempt: &Attempt) -> Result<TradeAssemblyService, String> {
        if !self.local_identity.uses_local_owner() {
            return Err("onboarding_local_owner_required".into());
        }
        let owner =
            tokio::runtime::Handle::current().block_on(self.local_identity.current_identity())?;
        if attempt
            .actor
            .as_ref()
            .is_some_and(|actor| actor != &owner.stable_identity_id)
        {
            return Err("onboarding_identity_changed".into());
        }
        register_verified_identity(&self.service, &owner)?;
        Ok(authenticated_service(&self.service, &owner))
    }

    fn connection(&self, id: &str) -> Result<Value, String> {
        let _guard = self.operation.lock().map_err(|_| "onboarding_busy")?;
        let _process_guard = self.process_lock()?;
        let mut attempt = self.read(id)?.ok_or("onboarding_not_found")?;
        if attempt.cancelled || now_ms() >= attempt.expires_at_ms {
            return Err("onboarding_attempt_ended".into());
        }
        let service = self.bound_local_service(&attempt)?;
        if (!attempt.api_keys_requested && !attempt.saved_paper_token_requested)
            || attempt.instance_ref.is_none()
        {
            return Ok(json!({"onboardingId":attempt.id,"mode":attempt.mode,
                "environment":attempt.environment,"instance":null}));
        }
        let saved = attempt.saved_paper_token_requested;
        let _oauth_guard = if saved {
            Some(service.saved_paper_token_lock(attempt.instance_ref.as_deref().unwrap())?)
        } else {
            None
        };
        let instance = self.reconcile_local_credentials(&service, &mut attempt, saved)?;
        Ok(json!({"onboardingId":attempt.id,"mode":attempt.mode,
            "environment":attempt.environment,"instance":instance}))
    }

    fn api_keys(&self, id: &str, body: &[u8]) -> Result<Value, String> {
        let _guard = self.operation.lock().map_err(|_| "onboarding_busy")?;
        let _process_guard = self.process_lock()?;
        let mut attempt = self.read(id)?.ok_or("onboarding_not_found")?;
        let service = self.bound_local_service(&attempt)?;
        if !attempt.api_keys_requested || attempt.saved_paper_token_requested {
            return Err("onboarding_broker_prepare_required".into());
        }
        let fields = api_key_fields(body)?;
        let projection = api_key_connection(&service, &attempt)?;
        if projection["activePackageSha256"].as_str()
            != fields.get("package_sha256").map(String::as_str)
        {
            return Err("onboarding_plugin_package_mismatch".into());
        }
        if attempt.api_keys_stored {
            return Err("onboarding_credentials_already_configured".into());
        }
        if attempt.api_key_intent.is_some() {
            let resumed = self.reconcile_api_keys(&service, &mut attempt)?;
            if attempt.api_keys_stored {
                return Ok(resumed);
            }
        } else {
            if projection["credentialStatus"]["configured"] == true {
                return Err("onboarding_credentials_already_configured".into());
            }
            attempt.api_key_intent = Some(ApiKeyIntent {
                package_sha256: fields["package_sha256"].clone(),
                prior_revision_ref: projection["credentialStatus"]["revisionRef"]
                    .as_str()
                    .ok_or("onboarding_credentials_status_failed")?
                    .into(),
                enable_sequence: 0,
            });
            // A crash after this write must never authorize adopting unrelated keys.
            self.save(&attempt)?;
        }
        let intent = attempt.api_key_intent.as_ref().unwrap();
        let stored = service.store_browser_onboarding_credentials(
            attempt.instance_ref.as_deref().unwrap(),
            &attempt.id,
            &intent.package_sha256,
            &intent.prior_revision_ref,
            json!({"api_key":fields["api_key"],"api_secret":fields["api_secret"]}),
        );
        if stored.status != 200 {
            return Err("onboarding_credentials_store_failed".into());
        }
        self.reconcile_api_keys(&service, &mut attempt)
    }

    fn saved_paper_token(&self, id: &str, body: &[u8]) -> Result<Value, String> {
        let _guard = self.operation.lock().map_err(|_| "onboarding_busy")?;
        let _process_guard = self.process_lock()?;
        let mut attempt = self.read(id)?.ok_or("onboarding_not_found")?;
        let service = self.bound_local_service(&attempt)?;
        if !attempt.saved_paper_token_requested || attempt.api_keys_requested {
            return Err("onboarding_broker_prepare_required".into());
        }
        let fields = saved_paper_token_fields(body)?;
        let instance = attempt
            .instance_ref
            .as_deref()
            .ok_or("onboarding_broker_prepare_required")?;
        let _oauth_guard = service.saved_paper_token_lock(instance)?;
        let projection = api_key_connection(&service, &attempt)?;
        if fields["onboarding_id"] != attempt.id || fields["instance_ref"] != instance {
            return Err("onboarding_restore_binding_mismatch".into());
        }
        if projection["activePackageSha256"] != fields["package_sha256"] {
            return Err("onboarding_plugin_package_mismatch".into());
        }
        if let Some(intent) = &attempt.saved_paper_token_intent {
            if intent.package_sha256 != fields["package_sha256"]
                || intent.prior_revision_ref != fields["prior_revision_ref"]
            {
                return Err("onboarding_restore_binding_mismatch".into());
            }
            let resumed = self.reconcile_local_credentials(&service, &mut attempt, true)?;
            if attempt.saved_paper_token_stored {
                return Ok(resumed);
            }
        } else {
            if projection["credentialStatus"]["configured"] == true {
                return Err("onboarding_credentials_already_configured".into());
            }
            if projection["credentialStatus"]["revisionRef"] != fields["prior_revision_ref"] {
                return Err("onboarding_restore_binding_mismatch".into());
            }
            attempt.saved_paper_token_intent = Some(ApiKeyIntent {
                package_sha256: fields["package_sha256"].clone(),
                prior_revision_ref: fields["prior_revision_ref"].clone(),
                enable_sequence: 0,
            });
            self.save(&attempt)?;
        }
        let intent = attempt.saved_paper_token_intent.as_ref().unwrap();
        let stored = service.store_browser_saved_paper_token(
            attempt.instance_ref.as_deref().unwrap(),
            &attempt.id,
            &intent.package_sha256,
            &intent.prior_revision_ref,
            fields["access_token"].clone(),
        );
        if stored.status != 200 {
            return Err("onboarding_credentials_store_failed".into());
        }
        self.reconcile_local_credentials(&service, &mut attempt, true)
    }

    fn reconcile_api_keys(
        &self,
        service: &TradeAssemblyService,
        attempt: &mut Attempt,
    ) -> Result<Value, String> {
        self.reconcile_local_credentials(service, attempt, false)
    }

    fn reconcile_local_credentials(
        &self,
        service: &TradeAssemblyService,
        attempt: &mut Attempt,
        saved: bool,
    ) -> Result<Value, String> {
        let projection = api_key_connection(service, attempt)?;
        let intent = if saved {
            &attempt.saved_paper_token_intent
        } else {
            &attempt.api_key_intent
        };
        if saved && attempt.saved_paper_token_stored && intent.is_none() {
            return Err("onboarding_credentials_receipt_invalid".into());
        }
        if intent.is_none() || (!saved && attempt.api_keys_stored) {
            return Ok(projection);
        }
        let intent = intent.as_ref().unwrap();
        let operation = if saved {
            "saved-paper-token"
        } else {
            "api-keys"
        };
        let method = if saved { "oauth" } else { "api-key" };
        if projection["activePackageSha256"] != intent.package_sha256 {
            return Err("onboarding_plugin_package_mismatch".into());
        }
        let instance = attempt.instance_ref.clone().unwrap();
        let envelope = crate::control_plane::http_envelope(
            "browser_onboarding_ui",
            "POST",
            &format!("/plugins/instances/{instance}/credentials"),
            &json!({"idempotency_key":format!("{}:{operation}",attempt.id)}),
        )?;
        let receipt = service
            .runtime()
            .storage
            .get_json(crate::control_plane::COMMAND_NS, &envelope.command_id)?
            .map(serde_json::from_value::<crate::control_plane::ControlPlaneCommandRecord>)
            .transpose()
            .map_err(|_| "onboarding_credentials_receipt_invalid")?;
        let Some(receipt) = receipt.filter(|r| r.response_status == Some(200)) else {
            if projection["credentialStatus"]["configured"] == true
                || projection["credentialStatus"]["revisionRef"] != intent.prior_revision_ref
            {
                return Err("onboarding_credentials_outcome_unknown".into());
            }
            return Ok(projection);
        };
        let binding = &receipt.envelope.payload_preview;
        let stored = receipt
            .response_body
            .as_ref()
            .ok_or("onboarding_credentials_receipt_invalid")?;
        if receipt.envelope.source_interface != "browser_onboarding_ui"
            || receipt.envelope.command_name != envelope.command_name
            || receipt.envelope.target_object != envelope.target_object
            || receipt.envelope.authority.actor.as_str()
                != attempt
                    .actor
                    .as_deref()
                    .ok_or("onboarding_identity_changed")?
            || receipt.envelope.authority.account_mode != "paper"
            || binding["onboardingId"] != attempt.id
            || binding["instanceRef"] != instance
            || binding["packageSha256"] != intent.package_sha256
            || binding["priorRevisionRef"] != intent.prior_revision_ref
            || stored["instanceRef"] != instance
            || stored["credentialStatus"]["configured"] != true
            || stored["credentialStatus"]["authenticationMethod"] != method
            || stored["credentialStatus"]["revisionRef"]
                != projection["credentialStatus"]["revisionRef"]
            || projection["credentialStatus"]["configured"] != true
            || projection["credentialStatus"]["authenticationMethod"] != method
        {
            return Err("onboarding_credentials_receipt_mismatch".into());
        }
        if saved && attempt.saved_paper_token_stored {
            return Ok(projection);
        }
        if projection["enabled"] != true {
            let sequence = intent
                .enable_sequence
                .checked_add(1)
                .ok_or("onboarding_state_invalid")?;
            if saved {
                attempt
                    .saved_paper_token_intent
                    .as_mut()
                    .unwrap()
                    .enable_sequence = sequence;
            } else {
                attempt.api_key_intent.as_mut().unwrap().enable_sequence = sequence;
            }
            self.save(attempt)?;
            let enabled = service.handle_http_from_source(
                "browser_onboarding_ui",
                "POST",
                &format!("/plugins/instances/{instance}:enable"),
                json!({"idempotency_key":format!("{}:{operation}-enable:{sequence}",attempt.id)}),
            );
            if enabled.status != 200 {
                return Err("onboarding_instance_enable_failed".into());
            }
        }
        if saved {
            attempt.saved_paper_token_stored = true;
        } else {
            attempt.api_keys_stored = true;
        }
        attempt.last_error = None;
        self.save(attempt)?;
        api_key_connection(service, attempt)
    }

    fn check_entitlement(&self, token: &str) -> Result<Value, String> {
        let profile = self
            .profile
            .as_ref()
            .ok_or("relay_connection_profile_required")?;
        if profile.entitlement_checks.is_empty() {
            return Err("relay_entitlement_configuration_required".into());
        }
        let endpoint = format!(
            "{}/v1/entitlement-decisions",
            profile.hub_base_url.trim_end_matches('/')
        );
        let config = profile.clone();
        let token = token.to_owned();
        std::thread::spawn(move||{
            let client=reqwest::blocking::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(10)).build().map_err(|_|"hub_unavailable")?;
            let mut permitted=true;
            for check in &config.entitlement_checks {
                let response=client.post(&endpoint).bearer_auth(&token).json(&json!({"product_id":check.product_id,"capability":check.capability,"scope":"account"})).send().map_err(|_|"hub_unavailable")?;
                if !response.status().is_success(){return Err("hub_entitlement_check_failed".to_string());}
                use std::io::Read;
                let mut bytes=Vec::new();response.take(16385).read_to_end(&mut bytes).map_err(|_|"hub_response_invalid")?;
                if bytes.len()>16384 {return Err("hub_response_invalid".into());}
                let value:Value=serde_json::from_slice(&bytes).map_err(|_|"hub_response_invalid")?;
                permitted &= value["permitted"]==true;
            }
            Ok(json!({"permitted":permitted,"status":if permitted {"verified"} else {"entitlement_required"},"checkoutUrl":config.checkout_url}))
        }).join().map_err(|_|"hub_unavailable")?
    }

    fn render(&self, id: &str) -> Result<String, String> {
        let _guard = self.operation.lock().map_err(|_| "onboarding_busy")?;
        let _process_guard = self.process_lock()?;
        let attempt = self.read(id)?.ok_or("onboarding_not_found")?;
        if attempt.cancelled || now_ms() >= attempt.expires_at_ms {
            return Ok(document(
                "This connection attempt has ended. Ask your agent to start a new connection.",
                "",
            ));
        }
        let progress = match self.public_status(&attempt) {
            Ok(value) => value,
            Err(code) => {
                return Ok(document(
                    error_message(&code),
                    &format!("<p><a href=\"{id}\">Retry connection status</a></p>"),
                ))
            }
        };
        let status = progress["identity"].clone();
        let local_credentials = attempt.api_keys_requested || attempt.saved_paper_token_requested;
        let mut links = String::new();
        if let Some(code) = attempt.last_error.as_ref() {
            links.push_str(&format!(
                "<p role=alert>{} Error code: <code>{}</code>.</p>",
                error_message(code),
                escape(code)
            ));
        }
        // Never reload a signed-in page while the user is choosing its next
        // action. Reloading here races browser clicks and assistive controls.
        if !local_credentials
            && (progress["hubAuthenticated"] != true
                || (attempt.connection_id.is_some() && progress["ready"] != true))
        {
            links.push_str("<meta http-equiv=refresh content=5>");
        }
        if attempt.mode == "paper" && attempt.connection_id.is_none() {
            if attempt.saved_paper_token_requested {
                let service = self.bound_local_service(&attempt)?;
                let projection = api_key_connection(&service, &attempt)?;
                if projection["credentialStatus"]["configured"] != true {
                    links.push_str(&saved_paper_token_form(id, &projection));
                }
            } else if attempt.api_keys_requested {
                let service = self.bound_local_service(&attempt)?;
                let projection = api_key_connection(&service, &attempt)?;
                if projection["credentialStatus"]["configured"] != true {
                    links.push_str(&api_key_form(id, &projection));
                }
            } else if attempt.actor.is_none() {
                links.push_str(&format!("<form method=post action=\"{id}/paper-token-restore\"><button>Restore a saved paper OAuth grant</button></form>"));
                links.push_str(&format!("<form method=post action=\"{id}/paper-keys\"><button>Connect with local paper API keys</button></form>"));
            }
        }
        if let Some(url) = status["browserUrl"].as_str() {
            links.push_str(&format!(
                "<p><a href=\"{}\" rel=\"noreferrer\">Continue sign-in</a></p>",
                escape(url)
            ));
        }
        if let Some(url) = attempt
            .authorization_url
            .as_ref()
            .filter(|_| progress["brokerConnected"] != true)
        {
            links.push_str(&format!(
                "<p><a href=\"{}\" rel=\"noreferrer\">Authorize broker connection</a></p>",
                escape(url)
            ));
        }
        if !local_credentials
            && attempt.connection_id.is_none()
            && progress["hubAuthenticated"] == true
        {
            let (service, _) = self.bound_service(&attempt)?;
            let (form, prepared) =
                broker_setup_form(&service, id, attempt.instance_ref.as_deref())?;
            links.push_str(&form);
            if prepared && attempt.mode == "paper" {
                if let Ok(projection) = api_key_connection(&service, &attempt) {
                    if projection["credentialStatus"]["configured"] != true {
                        links.push_str(&api_key_form(id, &projection));
                    }
                }
            }
        }
        if let Some(url) = progress["relay"]["checkoutUrl"]
            .as_str()
            .filter(|_| progress["relay"]["permitted"] != true)
        {
            links.push_str(&format!(
                "<p><a href=\"{}\" rel=noreferrer>Set up Relay subscription</a></p>",
                escape(url)
            ));
        }
        if !local_credentials && progress["hubAuthenticated"] != true {
            links.push_str(&format!("<form method=post target=_blank action=\"{id}/login\"><button>Sign in to TradeAssembly</button></form>"));
        }
        let controls=format!("<p>Broker account: {}. Deployment: {}.</p>{links}<form method=post action=\"{id}/cancel\"><button>Cancel setup</button></form>",escape(&attempt.mode),escape(&attempt.environment));
        Ok(document(
            if progress["ready"] == true {
                "Connection verified. You can return to your agent."
            } else {
                match progress["status"].as_str() {
                    _ if attempt.saved_paper_token_requested && !attempt.saved_paper_token_stored => "Restore the saved paper OAuth grant. First broker consent is a separate operation.",
                    _ if attempt.api_keys_requested && !attempt.api_keys_stored => "Enter paper API keys. They remain in this Core installation.",
                    Some("warden_unavailable") => "Your broker is connected. Ask your agent to start the local policy service, then return here.",
                    Some("entitlement_required") => "Your broker is connected. Relay access is still required to complete this setup.",
                    _ if progress["hubAuthenticated"] == true && attempt.connection_id.is_some() => "Finish broker authorization in the other tab. This page checks the connection automatically.",
                    _ if progress["hubAuthenticated"] == true => "Signed in. Connect your broker to continue.",
                    _ if status["status"] == "pending" => "Finish sign-in in the other tab. This page checks automatically.",
                    _ => "Sign in to TradeAssembly to connect your broker. Your local Core installation remains independent of this hosted connection.",
                }
            },
            &controls,
        ))
    }

    fn browser_action(
        &self,
        id: &str,
        action: &str,
        scopes: Vec<String>,
    ) -> Result<Option<String>, String> {
        let _guard = self.operation.lock().map_err(|_| "onboarding_busy")?;
        let _process_guard = self.process_lock()?;
        let mut attempt = self.read(id)?.ok_or("onboarding_not_found")?;
        if attempt.cancelled || now_ms() >= attempt.expires_at_ms {
            return Err("onboarding_attempt_ended".into());
        }
        match action {
            "login" => {
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                let started = tokio::runtime::Handle::current().block_on(
                    self.bootstrap.start_with_browser_opener(move |url| {
                        let _ = tx.send(url);
                        Ok(())
                    }),
                )?;
                if let Some(url) = started["browserUrl"].as_str() {
                    return Ok(Some(url.to_string()));
                }
                if started["status"] == "pending" {
                    return Ok(rx.recv_timeout(std::time::Duration::from_secs(5)).ok());
                }
            }
            "cancel" => {
                self.cancel(&mut attempt)?;
            }
            "paper-keys" | "paper-token-restore" => {
                let saved = action == "paper-token-restore";
                if (saved && attempt.api_keys_requested)
                    || (!saved && attempt.saved_paper_token_requested)
                {
                    return Err("onboarding_restore_binding_mismatch".into());
                }
                if attempt.mode != "paper" || attempt.connection_id.is_some() {
                    return Err("onboarding_api_keys_paper_required".into());
                }
                let service = self.bound_local_service(&attempt)?;
                let owner = tokio::runtime::Handle::current()
                    .block_on(self.local_identity.current_identity())?;
                let instance = attempt
                    .instance_ref
                    .clone()
                    .unwrap_or_else(|| "alpaca-paper".into());
                tool_payload(
                    &service,
                    "tradeassembly.broker.connect",
                    json!({"instanceRef":instance,"mode":"paper","idempotency_key":format!("{}:prepare",attempt.id)}),
                )?;
                attempt.instance_ref = Some(instance);
                attempt.actor = Some(owner.stable_identity_id);
                if saved {
                    let _oauth_guard =
                        service.saved_paper_token_lock(attempt.instance_ref.as_deref().unwrap())?;
                    api_key_connection(&service, &attempt)?;
                    attempt.saved_paper_token_requested = true;
                    self.save(&attempt)?;
                } else {
                    attempt.api_keys_requested = true;
                    self.save(&attempt)?;
                }
                return Ok(None);
            }
            "broker" | "authorize" => {
                if attempt.api_keys_requested || attempt.saved_paper_token_requested {
                    return Err("onboarding_credentials_already_configured".into());
                }
                if attempt.connection_id.is_some() {
                    return Ok(attempt.authorization_url.clone());
                }
                let (service, identity) = self.bound_service(&attempt)?;
                let instance = if let Some(instance) = attempt.instance_ref.as_ref() {
                    if service.broker_connection_descriptor(instance).is_err() {
                        tool_payload(
                            &service,
                            "tradeassembly.broker.connect",
                            json!({"instanceRef":instance,"mode":attempt.mode,
                                "idempotency_key":format!("{}:prepare",attempt.id)}),
                        )?;
                    }
                    service.broker_connection_descriptor(instance)?;
                    instance.clone()
                } else {
                    let prepared = tool_payload(
                        &service,
                        "tradeassembly.broker.connect",
                        json!({"mode":attempt.mode,"idempotency_key":format!("{}:prepare",attempt.id)}),
                    )?;
                    prepared["instanceRef"]
                        .as_str()
                        .ok_or("onboarding_broker_prepare_failed")?
                        .to_owned()
                };
                attempt.instance_ref = Some(instance.clone());
                attempt.actor = Some(identity.stable_identity_id);
                self.save(&attempt)?;
                if action == "broker" {
                    return Ok(None);
                }
                // User selects scopes rendered from the installed plugin descriptor.
                let started = tool_payload(
                    &service,
                    "tradeassembly.plugin.oauth.start",
                    json!({"instanceRef":instance,"environment":attempt.environment,"scopes":scopes,"idempotency_key":format!("{}:authorize",attempt.id)}),
                )?;
                let connection = started["connectionId"]
                    .as_str()
                    .ok_or("onboarding_broker_authorization_failed")?;
                attempt.connection_id = Some(connection.to_owned());
                attempt.authorization_url = Some(
                    started["authorizationUrl"]
                        .as_str()
                        .ok_or("onboarding_authorization_url_missing")?
                        .to_owned(),
                );
                self.save(&attempt)?;
                return Ok(attempt.authorization_url);
            }
            _ => return Err("onboarding_action_invalid".into()),
        }
        Ok(None)
    }
}

/// A requested instance name is not proof that preparation has completed.
/// Missing or inaccessible instances must offer the normal preparation action;
/// rendering never creates an instance or starts an OAuth handoff.
fn broker_setup_form(
    service: &TradeAssemblyService,
    id: &str,
    instance: Option<&str>,
) -> Result<(String, bool), String> {
    let descriptor = match instance.map(|instance| service.broker_connection_descriptor(instance)) {
        Some(Ok(descriptor)) => Some(descriptor),
        Some(Err(code)) if code != "onboarding_instance_unavailable" => return Err(code),
        _ => None,
    };
    let Some(descriptor) = descriptor else {
        return Ok((
            format!(
                "<form method=post action=\"{}/broker\"><button>Connect broker</button></form>",
                escape(id)
            ),
            false,
        ));
    };
    let mut form = format!("<form method=post target=_blank action=\"{}/authorize\"><fieldset><legend>Broker permissions</legend>", escape(id));
    for scope in descriptor["scopes"].as_array().into_iter().flatten() {
        if let (Some(key), Some(label)) = (scope["id"].as_str(), scope["label"].as_str()) {
            form.push_str(&format!(
                "<p><label><input type=checkbox name=scope value=\"{}\">{}</label></p>",
                escape(key),
                escape(label)
            ));
        }
    }
    form.push_str("</fieldset><button>Continue to broker authorization</button></form>");
    Ok((form, true))
}

fn api_key_connection(service: &TradeAssemblyService, attempt: &Attempt) -> Result<Value, String> {
    if attempt.cancelled || now_ms() >= attempt.expires_at_ms {
        return Err("onboarding_attempt_ended".into());
    }
    if attempt.mode != "paper" {
        return Err("onboarding_api_keys_paper_required".into());
    }
    if attempt.connection_id.is_some() {
        return Err("onboarding_oauth_pending".into());
    }
    let instance_ref = attempt
        .instance_ref
        .as_deref()
        .ok_or("onboarding_broker_prepare_required")?;
    let response = service.handle_http_from_source(
        "browser_onboarding_ui",
        "GET",
        &format!("/plugins/instances/{instance_ref}"),
        json!({}),
    );
    if response.status != 200 {
        return Err("onboarding_instance_unavailable".into());
    }
    let record = response.body;
    let configuration = &record["configuration"];
    if record["accountMode"] != "paper"
        || record["pluginRef"] != "tradeassembly.alpaca"
        || configuration["tradingApiUrl"] != "https://paper-api.alpaca.markets"
        || configuration
            .get("dataApiUrl")
            .is_some_and(|v| v != "https://data.alpaca.markets")
        || configuration.get("mode").is_some_and(|v| v != "paper")
        || configuration
            .get("account_mode")
            .is_some_and(|v| v != "paper")
    {
        return Err("onboarding_api_keys_configuration_invalid".into());
    }
    let package = record["activePackageSha256"]
        .as_str()
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("onboarding_plugin_package_required")?;
    let status = service.handle_http_from_source(
        "browser_onboarding_ui",
        "GET",
        &format!("/plugins/instances/{instance_ref}/credentials"),
        json!({}),
    );
    if status.status != 200 {
        return Err("onboarding_credentials_status_failed".into());
    }
    Ok(
        json!({"instanceRef":instance_ref,"pluginRef":record["pluginRef"],"activePackageSha256":package,
        "accountMode":"paper","enabled":record["enabled"],"credentialStatus":status.body["credentialStatus"]}),
    )
}

fn api_key_fields(body: &[u8]) -> Result<std::collections::BTreeMap<String, String>, String> {
    if body.len() > 8192 {
        return Err("onboarding_credentials_invalid".into());
    }
    let mut fields = std::collections::BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(body) {
        if !matches!(key.as_ref(), "api_key" | "api_secret" | "package_sha256")
            || fields
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return Err("onboarding_credentials_invalid".into());
        }
    }
    if fields.len() != 3
        || ["api_key", "api_secret"].iter().any(|key| {
            fields.get(*key).is_none_or(|v| {
                v.is_empty() || v.len() > 4096 || v.bytes().any(|b| b.is_ascii_control())
            })
        })
    {
        return Err("onboarding_credentials_invalid".into());
    }
    Ok(fields)
}

fn saved_paper_token_fields(
    body: &[u8],
) -> Result<std::collections::BTreeMap<String, String>, String> {
    if body.len() > 8192 {
        return Err("onboarding_credentials_invalid".into());
    }
    let mut fields = std::collections::BTreeMap::new();
    for (key, value) in url::form_urlencoded::parse(body) {
        if !matches!(
            key.as_ref(),
            "access_token"
                | "package_sha256"
                | "instance_ref"
                | "prior_revision_ref"
                | "onboarding_id"
        ) || value.is_empty()
            || value.len() > 4096
            || value.bytes().any(|b| b.is_ascii_control())
            || fields
                .insert(key.into_owned(), value.into_owned())
                .is_some()
        {
            return Err("onboarding_credentials_invalid".into());
        }
    }
    if fields.len() != 5
        || fields["access_token"]
            .bytes()
            .any(|b| !b.is_ascii_graphic())
    {
        return Err("onboarding_credentials_invalid".into());
    }
    Ok(fields)
}

fn saved_paper_token_form(id: &str, projection: &Value) -> String {
    format!("<form method=post action=\"{}/saved-paper-token\"><fieldset><legend>Restore a saved paper OAuth grant</legend><input type=hidden name=onboarding_id value=\"{}\"><input type=hidden name=instance_ref value=\"{}\"><input type=hidden name=package_sha256 value=\"{}\"><input type=hidden name=prior_revision_ref value=\"{}\"><p><label>Saved paper access token <input type=password name=access_token required autocomplete=off maxlength=4096></label></p></fieldset><button>Restore saved paper grant locally</button></form>", escape(id), escape(id), escape(projection["instanceRef"].as_str().unwrap_or_default()), escape(projection["activePackageSha256"].as_str().unwrap_or_default()), escape(projection["credentialStatus"]["revisionRef"].as_str().unwrap_or_default()))
}

fn api_key_form(id: &str, projection: &Value) -> String {
    format!("<form method=post action=\"{}/api-keys\"><fieldset><legend>Connect with paper API keys</legend><input type=hidden name=package_sha256 value=\"{}\"><p><label>Paper API key <input type=password name=api_key required autocomplete=off maxlength=4096></label></p><p><label>Paper API secret <input type=password name=api_secret required autocomplete=off maxlength=4096></label></p></fieldset><button>Save paper connection locally</button></form>", escape(id),escape(projection["activePackageSha256"].as_str().unwrap_or_default()))
}

fn safe_json(status: StatusCode, value: Value) -> Response {
    let mut response = (status, axum::Json(value)).into_response();
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

async fn connection(
    State(state): State<BrowserOnboarding>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !valid_host(&state, &headers, false) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match tokio::task::spawn_blocking(move || state.connection(&id)).await {
        Ok(Ok(value)) => safe_json(StatusCode::OK, value),
        Ok(Err(code)) => safe_json(
            StatusCode::BAD_REQUEST,
            json!({"error":{"code":machine_code(&code)}}),
        ),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn machine_code(code: &str) -> String {
    if !code.is_empty()
        && code.len() <= 128
        && code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        code.to_owned()
    } else {
        "onboarding_operation_failed".into()
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn document(message: &str, controls: &str) -> String {
    format!("<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>Connect TradeAssembly</title><h1>Connect TradeAssembly</h1><p>{}</p>{controls}<p>Broker credentials entered here stay in your local credential store. Never paste credentials into agent chat.</p></html>",escape(message))
}
fn secured(html: String) -> Response {
    let mut response = Html(html).into_response();
    for (key, value) in [
        ("cache-control", "no-store"),
        // Chrome sends Origin: null for form POSTs under no-referrer. Keep
        // same-origin mutations verifiable without leaking the capability URL
        // to external identity/broker pages.
        ("referrer-policy", "same-origin"),
        (
            "content-security-policy",
            "default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
        ("x-content-type-options", "nosniff"),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(key),
            axum::http::HeaderValue::from_static(value),
        );
    }
    response
}
async fn page(
    State(state): State<BrowserOnboarding>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !valid_host(&state, &headers, false) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match tokio::task::spawn_blocking(move || state.render(&id)).await {
        Ok(Ok(html)) => secured(html),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn action(
    State(state): State<BrowserOnboarding>,
    Path((id, action)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !valid_host(&state, &headers, true) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let redirect = state
        .listener
        .lock()
        .ok()
        .and_then(|v| v.clone())
        .map(|v| format!("{v}/{id}"));
    if body.len() > 8192 {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    if action == "api-keys" || action == "saved-paper-token" {
        let wants_json =
            headers.get("accept").and_then(|v| v.to_str().ok()) == Some("application/json");
        let result = tokio::task::spawn_blocking(move || {
            if action == "saved-paper-token" {
                state.saved_paper_token(&id, &body)
            } else {
                state.api_keys(&id, &body)
            }
        })
        .await;
        return match result {
            Ok(Ok(value)) if wants_json => safe_json(StatusCode::OK, value),
            Ok(Ok(_)) => {
                axum::response::Redirect::to(redirect.as_deref().unwrap_or("/")).into_response()
            }
            Ok(Err(code)) if wants_json => safe_json(
                StatusCode::BAD_REQUEST,
                json!({"error":{"code":machine_code(&code)}}),
            ),
            Ok(Err(code)) => secured(document(
                error_message(&code),
                &format!("<p>Error code: <code>{}</code>.</p>", machine_code(&code)),
            )),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    if action == "paper-token-restore" && !body.is_empty() {
        return safe_json(
            StatusCode::BAD_REQUEST,
            json!({"error":{"code":"onboarding_credentials_invalid"}}),
        );
    }
    let scopes = url::form_urlencoded::parse(&body)
        .filter(|(key, _)| key == "scope")
        .map(|(_, value)| value.into_owned())
        .collect();
    match tokio::task::spawn_blocking(move || {
        let result = state.browser_action(&id, &action, scopes);
        let _guard = state.operation.lock().map_err(|_| "onboarding_busy".to_string())?;
        let _process_guard = state.process_lock()?;
        if let Some(mut attempt) = state.read(&id)? {
            attempt.last_error = result.as_ref().err().map(|code| machine_code(code));
            state.save(&attempt)?;
        }
        result
    }).await {
        // Complete the same-origin POST before navigating externally. A 303
        // to a provider is rejected by Chrome's form-action 'self' policy.
        Ok(Ok(Some(url))) => secured(document(
            "Opening the secure authorization page…",
            &format!("<meta http-equiv=refresh content=\"0;url={}\"><p><a href=\"{}\" rel=noreferrer>Continue to authorization</a></p>", escape(&url), escape(&url)),
        )),
        Ok(Ok(None)) => {
            axum::response::Redirect::to(redirect.as_deref().unwrap_or("/")).into_response()
        }
        Ok(Err(code)) => secured(document(
            error_message(&code),
            &format!("<p>Error code: <code>{}</code>. Return to the connection page or ask your agent to inspect onboarding status.</p>", machine_code(&code)),
        )),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
fn verification_arguments(attempt_id: &str, instance: &str, mode: &str) -> Value {
    // Each status poll requests fresh read-only account health. Reusing the
    // onboarding identity would replay an earlier failure after local repair.
    // Explicit broker commands retain their normal durable replay semantics.
    json!({"instanceRef":instance,"mode":mode,
        "idempotency_key":format!("{attempt_id}:verify:{:032x}",rand::random::<u128>())})
}

fn tool_payload(service: &TradeAssemblyService, name: &str, args: Value) -> Result<Value, String> {
    let response = service.call_mcp_tool(name, args);
    let value = &response["structuredContent"];
    if response["isError"] == true || value["error"].is_object() || value["error"].is_string() {
        let code = value["error"]["code"]
            .as_str()
            .or_else(|| value["error"].as_str())
            .unwrap_or("onboarding_tool_failed");
        // Only stable machine codes may cross the browser boundary.
        return Err(
            if code.len() <= 128
                && code
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            {
                code.into()
            } else {
                "onboarding_tool_failed".into()
            },
        );
    }
    Ok(value.clone())
}

fn error_message(code: &str) -> &'static str {
    match code {
        "plugin_oauth_relay_not_allowed"|"plugin_oauth_not_configured"|"connection_profile_invalid"|"connection_profile_unavailable"=>"This installation's connection settings need repair. Your credentials are not the problem; use the published installer or contact support.",
        "hosted_sign_in_required"|"oidc_session_required"|"plugin_oauth_identity_required"=>"Sign in using the connection page, then retry.",
        "bitwarden_session_required"|"bitwarden_session_store_unavailable"=>"Unlock your configured credential store, then retry sign-in. Do not share its password with your agent.",
        "onboarding_identity_changed"|"plugin_oauth_identity_changed"=>"The signed-in account changed. Start a new connection attempt for the selected account.",
        "relay_connection_profile_required"|"relay_entitlement_configuration_required"=>"Relay subscription setup is not configured for this release. Broker-only setup remains available; contact support for the Relay release.",
        "hub_unavailable"|"plugin_oauth_relay_unavailable"=>"The connection service could not be reached. Retry from this page when it is available.",
        "onboarding_busy"=>"Another connection operation is in progress. Retry shortly.",
        "plugin_oauth_start_outcome_unknown"|"plugin_oauth_attempt_ended"=>"This authorization attempt cannot safely resume. Ask your agent to cancel it and start a new connection. Do not reuse the old authorization tab.",
        _=>"The connection could not complete. Return to your agent with the reported error code; do not paste credentials or edit configuration.",
    }
}
fn valid_host(state: &BrowserOnboarding, headers: &HeaderMap, mutation: bool) -> bool {
    let base = state.listener.lock().ok().and_then(|v| v.clone());
    let Some(url) = base.and_then(|v| url::Url::parse(&v).ok()) else {
        return false;
    };
    let expected = format!("127.0.0.1:{}", url.port().unwrap_or(80));
    headers.get("host").and_then(|v| v.to_str().ok()) == Some(expected.as_str())
        && (!mutation
            || headers.get("origin").and_then(|v| v.to_str().ok())
                == Some(url.origin().ascii_serialization().as_str()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn verification_polls_use_fresh_keys_without_changing_account_or_mode() {
        let first = super::verification_arguments("attempt-1", "alpaca-paper", "paper");
        let retry = super::verification_arguments("attempt-1", "alpaca-paper", "paper");
        assert_ne!(first["idempotency_key"], retry["idempotency_key"]);
        for arguments in [first, retry] {
            assert_eq!(arguments["instanceRef"], "alpaca-paper");
            assert_eq!(arguments["mode"], "paper");
            assert!(arguments["idempotency_key"]
                .as_str()
                .unwrap()
                .starts_with("attempt-1:verify:"));
        }
    }
    use super::*;

    fn fixture() -> (tempfile::TempDir, BrowserOnboarding) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("runtime.db")
            .display()
            .to_string();
        let mut config = crate::runtime_config::RuntimeConfig::local(db);
        config.oidc_profile = "local_owner".into();
        config.oidc_session_path = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("session.bin")
            .display()
            .to_string();
        config.artifact_root = Some(
            dir.path()
                .canonicalize()
                .unwrap()
                .join("artifacts")
                .display()
                .to_string(),
        );
        let (runtime, _) = crate::runtime_config::RuntimeBuilder::new(config.clone())
            .with_finance_authority(Arc::new(crate::finance_authority::TestFinanceAuthority))
            .build()
            .unwrap();
        let service =
            TradeAssemblyService::from_runtime(config.database_path.as_ref().unwrap(), runtime);
        let manager = CliIdentityManager::new(config);
        let mut coordinator = BrowserOnboarding::new(&service, &manager);
        coordinator.profile = None;
        coordinator.profile_error = None;
        (dir, coordinator)
    }
    fn invoke(c: &BrowserOnboarding, name: &str, args: Value) -> Value {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { tokio::task::block_in_place(|| c.call(name, args)) })
    }
    fn start(c: &BrowserOnboarding) -> Value {
        invoke(
            c,
            "tradeassembly.onboarding.start",
            json!({"mode":"paper","environment":"staging","idempotency_key":"setup-1"}),
        )
    }

    fn install_paper_manifest(c: &BrowserOnboarding) {
        let response = c.service.handle_http(
            "GET",
            "/plugins/manifests/tradeassembly.simbroker",
            json!({}),
        );
        assert_eq!(response.status, 200);
        let mut manifest = response.body;
        manifest["pluginRef"] = json!("tradeassembly.alpaca");
        manifest["manifest"]["metadata"]["id"] = json!("tradeassembly.alpaca");
        manifest["package"] = json!({"packageSha256":"a".repeat(64)});
        manifest["manifest"]["configuration"] = json!({"fields":[
            {"id":"mode","inputType":"select","options":["paper","live"],"storageClass":"configuration","required":true},
            {"id":"tradingApiUrl","inputType":"url","storageClass":"configuration","required":true,"defaultByAccountMode":{"paper":"https://paper-api.alpaca.markets","live":"https://api.alpaca.markets"}},
            {"id":"api_key","inputType":"password","storageClass":"credential","required":true},
            {"id":"api_secret","inputType":"password","storageClass":"credential","required":true},
            {"id":"access_token","inputType":"password","storageClass":"credential","required":false}
        ]});
        manifest["manifest"]["health"]["requiredConfiguration"] = json!(["tradingApiUrl"]);
        manifest["manifest"]["health"]["requiredCredentials"] = json!([]);
        manifest["manifest"]["health"]["credentialAlternatives"] =
            json!([["api_key", "api_secret"], ["access_token"]]);
        let context = SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new("paper-manifest-fixture").unwrap(),
        );
        c.service
            .runtime()
            .plugins
            .install_manifest("tradeassembly.alpaca", manifest, &context)
            .unwrap();
    }

    fn in_runtime<T>(operation: impl FnOnce() -> T) -> T {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { tokio::task::block_in_place(operation) })
    }

    fn paper_attempt(c: &BrowserOnboarding) -> String {
        install_paper_manifest(c);
        let id = start(c)["onboardingId"].as_str().unwrap().to_string();
        in_runtime(|| c.browser_action(&id, "paper-keys", vec![])).unwrap();
        let attempt = c.read(&id).unwrap().unwrap();
        let service = in_runtime(|| c.bound_local_service(&attempt)).unwrap();
        assert_eq!(
            service
                .handle_http_from_source(
                    "fixture",
                    "POST",
                    "/plugins/instances/alpaca-paper:disable",
                    json!({"idempotency_key":"fixture-disable"})
                )
                .status,
            200
        );
        id
    }

    fn paper_form() -> String {
        format!(
            "api_key=PAPER_KEY_CANARY&api_secret=PAPER_SECRET_CANARY&package_sha256={}",
            "a".repeat(64)
        )
    }

    fn restart(c: &BrowserOnboarding) -> BrowserOnboarding {
        let config = c.local_identity.configuration().clone();
        let (runtime, _) = crate::runtime_config::RuntimeBuilder::new(config.clone())
            .with_finance_authority(Arc::new(crate::finance_authority::TestFinanceAuthority))
            .build()
            .unwrap();
        let service = TradeAssemblyService::from_runtime(c.service.db(), runtime);
        let mut resumed = BrowserOnboarding::new(&service, &CliIdentityManager::new(config));
        resumed.profile = None;
        resumed.profile_error = None;
        resumed
    }

    fn fail_write(c: &BrowserOnboarding, predicate: &str) {
        rusqlite::Connection::open(c.service.db()).unwrap().execute_batch(&format!(
            "CREATE TRIGGER onboarding_write_fault BEFORE INSERT ON tradeassembly_kv WHEN {predicate}
             BEGIN SELECT RAISE(ABORT, 'onboarding fixture write failure'); END;"
        )).unwrap();
    }

    fn clear_write_fault(c: &BrowserOnboarding) {
        rusqlite::Connection::open(c.service.db())
            .unwrap()
            .execute_batch("DROP TRIGGER onboarding_write_fault;")
            .unwrap();
    }

    fn saved_paper_attempt(c: &BrowserOnboarding) -> String {
        install_paper_manifest(c);
        let id = start(c)["onboardingId"].as_str().unwrap().to_owned();
        in_runtime(|| c.browser_action(&id, "paper-token-restore", vec![])).unwrap();
        let attempt = c.read(&id).unwrap().unwrap();
        let service = in_runtime(|| c.bound_local_service(&attempt)).unwrap();
        assert_eq!(
            service
                .handle_http_from_source(
                    "fixture",
                    "POST",
                    "/plugins/instances/alpaca-paper:disable",
                    json!({"idempotency_key":"saved-fixture-disable"})
                )
                .status,
            200
        );
        id
    }

    fn saved_paper_form(id: &str) -> String {
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("access_token", "SAVED_PAPER_TOKEN_CANARY")
            .append_pair("package_sha256", &"a".repeat(64))
            .append_pair("instance_ref", "alpaca-paper")
            .append_pair("prior_revision_ref", "credential-generation:0")
            .append_pair("onboarding_id", id)
            .finish()
    }

    #[test]
    fn first_consent_named_instance_renders_preparation_and_keeps_local_owner_authority() {
        let (_dir, c) = fixture();
        install_paper_manifest(&c);
        let started = invoke(
            &c,
            "tradeassembly.onboarding.start",
            json!({
            "instanceRef":"fresh-consent-paper", "mode":"paper", "environment":"staging",
            "idempotency_key":"first-consent-named-instance"}),
        );
        let id = started["onboardingId"].as_str().unwrap();
        let attempt = c.read(id).unwrap().unwrap();
        let local = in_runtime(|| c.bound_local_service(&attempt)).unwrap();
        let owner = in_runtime(|| {
            tokio::runtime::Handle::current().block_on(c.local_identity.current_identity())
        })
        .unwrap();
        // A distinct hosted actor only selects hosted OAuth custody. It must
        // never replace local ownership or finance authority for preparation.
        let service = local.with_hosted_connection(
            c.identity.configuration().clone(),
            "test-fixture-distinct-hosted-actor".into(),
            None,
        );
        for instance in [None, Some("fresh-consent-paper")] {
            let (form, prepared) = broker_setup_form(&service, id, instance).unwrap();
            assert!(!prepared);
            assert!(form.contains(&format!("{id}/broker")));
            assert!(!form.contains("/authorize"));
        }
        assert!(c
            .service
            .runtime()
            .plugins
            .get_instance("fresh-consent-paper")
            .unwrap()
            .is_none());
        let prepared = tool_payload(&service, "tradeassembly.broker.connect", json!({
            "instanceRef":"fresh-consent-paper", "mode":"paper", "idempotency_key":format!("{id}:prepare")})).unwrap();
        assert_eq!(prepared["instanceRef"], "fresh-consent-paper");
        let (form, prepared) =
            broker_setup_form(&service, id, Some("fresh-consent-paper")).unwrap();
        assert!(prepared);
        assert!(form.contains(&format!("{id}/authorize")));
        assert!(!form.contains(">Connect broker</button>"));
        let commands =
            crate::control_plane::list_commands(c.service.runtime().storage.as_ref()).unwrap();
        let command = commands
            .iter()
            .find(|r| r.envelope.idempotency_key.as_str() == format!("{id}:prepare"))
            .unwrap();
        assert_eq!(command.response_status, Some(200));
        assert_eq!(command.envelope.authority.actor, owner.stable_identity_id);
        assert_eq!(command.envelope.authority.account_mode, "paper");
        assert_eq!(command.envelope.source_interface, "mcp");
        assert!(!serde_json::to_string(&commands)
            .unwrap()
            .contains("distinct-hosted-actor"));
        assert!(c
            .service
            .runtime()
            .storage
            .list_json("plugin_oauth_attempts")
            .unwrap()
            .is_empty());
        assert!(
            !c.service
                .runtime()
                .credentials
                .status("fresh-consent-paper")
                .configured
        );
    }

    #[test]
    fn saved_paper_token_http_is_bound_local_oauth_and_replays_without_secret_evidence() {
        let (_dir, c) = fixture();
        let id = saved_paper_attempt(&c);
        let url = format!("{}/{}", c.ensure_listener().unwrap(), id);
        let origin = url::Url::parse(&url)
            .unwrap()
            .origin()
            .ascii_serialization();
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let form = saved_paper_form(&id);
        let page = client.get(&url).send().unwrap().text().unwrap();
        assert!(page.contains("Restore a saved paper OAuth grant"));
        assert!(page.contains("First broker consent is a separate operation"));
        for bad_origin in ["null", "https://evil.test", ""] {
            assert_eq!(
                client
                    .post(format!("{url}/saved-paper-token"))
                    .header("origin", bad_origin)
                    .body(form.clone())
                    .send()
                    .unwrap()
                    .status(),
                403
            );
        }
        assert_eq!(
            client
                .post(format!("{url}/saved-paper-token"))
                .header("origin", &origin)
                .header("host", "evil.test")
                .body(form.clone())
                .send()
                .unwrap()
                .status(),
            403
        );
        let other_capability = url.replace(
            url::Url::parse(&url)
                .unwrap()
                .path()
                .split('/')
                .nth(1)
                .unwrap(),
            "not-the-capability",
        );
        assert_eq!(
            client
                .post(format!("{other_capability}/saved-paper-token"))
                .header("origin", &origin)
                .body(form.clone())
                .send()
                .unwrap()
                .status(),
            404
        );
        for invalid in [
            form.replace(&"a".repeat(64), &"b".repeat(64)),
            form.replace(&id, &"b".repeat(64)),
            form.replace("alpaca-paper", "another-instance"),
            form.replace("credential-generation%3A0", "credential-generation%3A1"),
            format!("{form}&api_key=mixed"),
            format!("{form}&access_token=duplicate"),
            "access_token=only".into(),
        ] {
            let response = client
                .post(format!("{url}/saved-paper-token"))
                .header("origin", &origin)
                .header("accept", "application/json")
                .body(invalid)
                .send()
                .unwrap();
            assert_eq!(response.status(), 400, "{}", response.text().unwrap());
            assert!(
                !c.service
                    .runtime()
                    .credentials
                    .status("alpaca-paper")
                    .configured
            );
        }
        let response = client
            .post(format!("{url}/saved-paper-token"))
            .header("origin", &origin)
            .header("accept", "application/json")
            .body(form.clone())
            .send()
            .unwrap();
        assert_eq!(response.status(), 200);
        let restored: Value = response.json().unwrap();
        assert_eq!(
            restored["credentialStatus"]["authenticationMethod"],
            "oauth"
        );
        assert_eq!(
            restored["credentialStatus"]["revisionRef"],
            "credential-generation:1"
        );
        assert_eq!(restored["enabled"], true);
        let replay: Value = client
            .post(format!("{url}/saved-paper-token"))
            .header("origin", &origin)
            .header("accept", "application/json")
            .body(form)
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(replay, restored);
        let resumed = restart(&c);
        assert_eq!(
            in_runtime(|| resumed.connection(&id)).unwrap()["instance"],
            restored
        );
        assert_eq!(
            in_runtime(|| resumed.saved_paper_token(&id, saved_paper_form(&id).as_bytes()))
                .unwrap(),
            restored
        );
        let commands =
            crate::control_plane::list_commands(resumed.service.runtime().storage.as_ref())
                .unwrap();
        let stores: Vec<_> = commands
            .iter()
            .filter(|r| r.envelope.idempotency_key.as_str() == format!("{id}:saved-paper-token"))
            .collect();
        assert_eq!(stores.len(), 1);
        assert_eq!(stores[0].response_status, Some(200));
        // The word token in the public operation key is conservatively
        // redacted from previews. Restore that known, non-secret key to prove
        // the command digest contains public bindings only.
        let mut safe_binding = stores[0].envelope.payload_preview.clone();
        safe_binding["idempotency_key"] = json!(format!("{id}:saved-paper-token"));
        assert_eq!(
            stores[0].envelope.payload_hash,
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&safe_binding).unwrap())
            )
        );
        let token_hash = format!("{:x}", Sha256::digest(b"SAVED_PAPER_TOKEN_CANARY"));
        let evidence = serde_json::to_string(&commands).unwrap();
        assert!(!evidence.contains("CANARY") && !evidence.contains(&token_hash));
        let conn = rusqlite::Connection::open(resumed.service.db()).unwrap();
        let leaked: i64 = conn.query_row("SELECT count(*) FROM tradeassembly_kv WHERE value_json LIKE '%SAVED_PAPER_TOKEN_CANARY%' OR value_json LIKE ?1", [format!("%{token_hash}%")], |r| r.get(0)).unwrap();
        assert_eq!(leaked, 0);
        let attempt = resumed.read(&id).unwrap().unwrap();
        assert!(attempt.saved_paper_token_stored && !attempt.api_keys_stored);
        assert!(attempt.connection_id.is_none());
        assert!(crate::agent_runner::deployments(&resumed.service.runtime())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn saved_paper_token_cannot_mix_setup_modes_or_race_oauth_and_rejects_malformed_input() {
        let (_dir, c) = fixture();
        let id = saved_paper_attempt(&c);
        assert!(in_runtime(|| c.browser_action(&id, "paper-keys", vec![])).is_err());
        assert!(in_runtime(|| c.api_keys(&id, paper_form().as_bytes())).is_err());
        let service = in_runtime(|| c.bound_local_service(&c.read(&id).unwrap().unwrap())).unwrap();
        let lock = service.saved_paper_token_lock("alpaca-paper").unwrap();
        assert_eq!(
            in_runtime(|| c.saved_paper_token(&id, saved_paper_form(&id).as_bytes())).unwrap_err(),
            "plugin_oauth_lock_busy"
        );
        drop(lock);
        for invalid in [
            saved_paper_form(&id).replace("SAVED_PAPER_TOKEN_CANARY", ""),
            saved_paper_form(&id).replace("SAVED_PAPER_TOKEN_CANARY", "has+space"),
            saved_paper_form(&id).replace("SAVED_PAPER_TOKEN_CANARY", "%0A"),
            saved_paper_form(&id).replace("SAVED_PAPER_TOKEN_CANARY", "%FF"),
            saved_paper_form(&id).replace("SAVED_PAPER_TOKEN_CANARY", &"x".repeat(4097)),
            "x".repeat(8193),
        ] {
            assert!(saved_paper_token_fields(invalid.as_bytes()).is_err());
        }
        assert!(
            !c.service
                .runtime()
                .credentials
                .status("alpaca-paper")
                .configured
        );
    }

    #[test]
    fn saved_paper_token_restart_reconciles_only_a_successful_owned_receipt() {
        for fault in [
            "NEW.namespace='control_plane_commands' AND NEW.idempotency_key LIKE '%:saved-paper-token-enable:%'",
            "NEW.namespace='browser_onboarding_v1' AND json_extract(NEW.value_json,'$.savedPaperTokenStored')=1",
        ] {
            let (_dir, c) = fixture();
            let id = saved_paper_attempt(&c);
            fail_write(&c, fault);
            assert!(in_runtime(|| c.saved_paper_token(&id, saved_paper_form(&id).as_bytes())).is_err());
            clear_write_fault(&c);
            let resumed = restart(&c);
            let result = in_runtime(|| resumed.connection(&id)).unwrap();
            assert_eq!(result["instance"]["enabled"], true);
            assert_eq!(result["instance"]["credentialStatus"]["authenticationMethod"], "oauth");
            assert_eq!(result["instance"]["credentialStatus"]["revisionRef"], "credential-generation:1");
            assert!(resumed.read(&id).unwrap().unwrap().saved_paper_token_stored);
        }
    }

    #[test]
    fn saved_paper_token_failed_intent_has_no_effect_and_unreceipted_write_fails_closed() {
        for (fault, configured) in [
            ("NEW.namespace='browser_onboarding_v1' AND json_type(NEW.value_json,'$.savedPaperTokenIntent')='object'", false),
            ("NEW.namespace='control_plane_commands' AND NEW.idempotency_key LIKE '%:saved-paper-token' AND json_extract(NEW.value_json,'$.response_status')=200", true),
        ] {
            let (_dir, c) = fixture();
            let id = saved_paper_attempt(&c);
            fail_write(&c, fault);
            assert!(in_runtime(|| c.saved_paper_token(&id, saved_paper_form(&id).as_bytes())).is_err());
            assert_eq!(c.service.runtime().credentials.status("alpaca-paper").configured, configured);
            clear_write_fault(&c);
            let resumed = restart(&c);
            let result = in_runtime(|| resumed.saved_paper_token(&id, saved_paper_form(&id).as_bytes()));
            if configured {
                assert_eq!(result.unwrap_err(), "onboarding_credentials_outcome_unknown");
                assert_eq!(in_runtime(|| resumed.connection(&id)).unwrap_err(), "onboarding_credentials_outcome_unknown");
            } else { assert!(result.is_ok()); }
        }
    }

    #[test]
    fn saved_paper_token_rejects_pending_handoff_configuration_and_owner_changes() {
        for alteration in [
            "pending",
            "configuration",
            "owner",
            "live",
            "cancelled",
            "expired",
            "existing",
            "replacement",
        ] {
            let (_dir, c) = fixture();
            let id = saved_paper_attempt(&c);
            let mut attempt = c.read(&id).unwrap().unwrap();
            let service = in_runtime(|| c.bound_local_service(&attempt)).unwrap();
            match alteration {
                "pending" => {
                    c.service
                        .runtime()
                        .storage
                        .put_json(
                            "plugin_oauth_attempts",
                            "pending-fixture",
                            json!({"instanceRef":"alpaca-paper"}),
                            &SideEffectContext::new(
                                AuthorityContext::local_cli(),
                                IdempotencyKey::new("pending-fixture").unwrap(),
                            ),
                        )
                        .unwrap();
                }
                "configuration" => {
                    // Simulate an incompatible on-disk configuration; normal
                    // configuration authoring already rejects this mismatch.
                    let mut record = c
                        .service
                        .runtime()
                        .plugins
                        .get_instance("alpaca-paper")
                        .unwrap()
                        .unwrap();
                    record["configuration"]["tradingApiUrl"] = json!("https://api.alpaca.markets");
                    c.service
                        .runtime()
                        .plugins
                        .put_instance(
                            "alpaca-paper",
                            record,
                            &SideEffectContext::new(
                                AuthorityContext::local_cli(),
                                IdempotencyKey::new("invalid-url-fixture").unwrap(),
                            ),
                        )
                        .unwrap();
                }
                "owner" => {
                    attempt.actor = Some("unrelated-owner".into());
                    c.save(&attempt).unwrap();
                }
                "live" => {
                    attempt.mode = "live".into();
                    c.save(&attempt).unwrap();
                }
                "cancelled" => {
                    attempt.cancelled = true;
                    c.save(&attempt).unwrap();
                }
                "expired" => {
                    attempt.expires_at_ms = 0;
                    c.save(&attempt).unwrap();
                }
                "existing" | "replacement" => {
                    if alteration == "replacement" {
                        in_runtime(|| c.saved_paper_token(&id, saved_paper_form(&id).as_bytes()))
                            .unwrap();
                    }
                    assert_eq!(service.handle_http_from_source("fixture", "POST", "/plugins/instances/alpaca-paper/credentials",
                        json!({"credentials":{"access_token":"UNRELATED"},"idempotency_key":"unrelated-token"})).status, 200);
                }
                _ => unreachable!(),
            }
            assert!(
                in_runtime(|| c.saved_paper_token(&id, saved_paper_form(&id).as_bytes())).is_err(),
                "{alteration}"
            );
        }
    }

    #[test]
    fn paper_key_restart_finishes_failed_enable_or_attempt_save_without_replacing_keys() {
        for (fault, expected_error, enabled_before_restart) in [
            ("NEW.namespace='control_plane_commands' AND NEW.idempotency_key LIKE '%:api-keys-enable:%'", "onboarding_instance_enable_failed", false),
            ("NEW.namespace='browser_onboarding_v1' AND json_extract(NEW.value_json,'$.apiKeysStored')=1", "sqlite_operation_failed", true),
        ] {
            let (_dir, c) = fixture();
            let id = paper_attempt(&c);
            fail_write(&c, fault);
            let error = in_runtime(|| c.api_keys(&id, paper_form().as_bytes())).unwrap_err();
            if enabled_before_restart {
                assert!(!error.is_empty());
            } else {
                assert_eq!(error, expected_error);
            }
            let attempt = c.read(&id).unwrap().unwrap();
            assert!(!attempt.api_keys_stored);
            assert!(attempt.api_key_intent.is_some());
            let before = in_runtime(|| api_key_connection(&c.bound_local_service(&attempt).unwrap(), &attempt)).unwrap();
            assert_eq!(before["enabled"], enabled_before_restart);
            assert_eq!(before["credentialStatus"]["configured"], true);
            clear_write_fault(&c);
            let resumed = restart(&c);
            let result = in_runtime(|| resumed.connection(&id)).unwrap();
            assert_eq!(result["instance"]["enabled"], true);
            assert_eq!(result["instance"]["credentialStatus"]["revisionRef"], before["credentialStatus"]["revisionRef"]);
            assert!(resumed.read(&id).unwrap().unwrap().api_keys_stored);
            let repeated = in_runtime(|| resumed.connection(&id)).unwrap();
            assert_eq!(repeated, result);
            let commands = crate::control_plane::list_commands(resumed.service.runtime().storage.as_ref()).unwrap();
            let stores: Vec<_> = commands.iter().filter(|r| r.envelope.idempotency_key.as_str() == format!("{id}:api-keys")).collect();
            assert_eq!(stores.len(), 1);
            assert_eq!(stores[0].response_status, Some(200));
            // The digest is entirely reproducible from safe public metadata.
            assert_eq!(stores[0].envelope.payload_hash, format!("{:x}", Sha256::digest(serde_json::to_vec(&stores[0].envelope.payload_preview).unwrap())));
            assert!(!serde_json::to_string(&commands).unwrap().contains("CANARY"));
            assert!(crate::agent_runner::deployments(&resumed.service.runtime()).unwrap().is_empty());
        }
    }

    #[test]
    fn paper_key_intent_failure_precedes_credential_effect_and_can_retry_after_restart() {
        let (_dir, c) = fixture();
        let id = paper_attempt(&c);
        fail_write(&c, "NEW.namespace='browser_onboarding_v1' AND json_type(NEW.value_json,'$.apiKeyIntent')='object'");
        assert!(in_runtime(|| c.api_keys(&id, paper_form().as_bytes())).is_err());
        assert!(
            !c.service
                .runtime()
                .credentials
                .status("alpaca-paper")
                .configured
        );
        assert!(c.read(&id).unwrap().unwrap().api_key_intent.is_none());
        clear_write_fault(&c);
        let resumed = restart(&c);
        assert_eq!(
            in_runtime(|| resumed.api_keys(&id, paper_form().as_bytes())).unwrap()["enabled"],
            true
        );
    }

    #[test]
    fn paper_key_unreceipted_store_never_adopts_or_overwrites_keys_on_restart() {
        let (_dir, c) = fixture();
        let id = paper_attempt(&c);
        fail_write(&c, "NEW.namespace='control_plane_commands' AND NEW.idempotency_key LIKE '%:api-keys' AND json_extract(NEW.value_json,'$.response_status')=200");
        assert_eq!(
            in_runtime(|| c.api_keys(&id, paper_form().as_bytes())).unwrap_err(),
            "onboarding_credentials_store_failed"
        );
        assert!(
            c.service
                .runtime()
                .credentials
                .status("alpaca-paper")
                .configured
        );
        clear_write_fault(&c);
        let resumed = restart(&c);
        assert_eq!(
            in_runtime(|| resumed.connection(&id)).unwrap_err(),
            "onboarding_credentials_outcome_unknown"
        );
        assert_eq!(
            in_runtime(|| resumed.api_keys(
                &id,
                paper_form().replace("CANARY", "REPLACEMENT").as_bytes()
            ))
            .unwrap_err(),
            "onboarding_credentials_outcome_unknown"
        );
        let attempt = resumed.read(&id).unwrap().unwrap();
        assert!(!attempt.api_keys_stored);
        let projection = in_runtime(|| {
            api_key_connection(&resumed.bound_local_service(&attempt).unwrap(), &attempt)
        })
        .unwrap();
        assert_eq!(projection["enabled"], false);
        assert_eq!(
            projection["credentialStatus"]["revisionRef"],
            "credential-generation:1"
        );
    }

    #[test]
    fn paper_key_resume_rejects_unrelated_credential_generation_and_changed_actor() {
        let (_dir, c) = fixture();
        let id = paper_attempt(&c);
        fail_write(&c, "NEW.namespace='control_plane_commands' AND NEW.idempotency_key LIKE '%:api-keys-enable:%'");
        assert!(in_runtime(|| c.api_keys(&id, paper_form().as_bytes())).is_err());
        clear_write_fault(&c);
        let mut attempt = c.read(&id).unwrap().unwrap();
        let service = in_runtime(|| c.bound_local_service(&attempt)).unwrap();
        let replaced = service.handle_http_from_source("fixture", "POST", "/plugins/instances/alpaca-paper/credentials",
            json!({"credentials":{"api_key":"unrelated-key","api_secret":"unrelated-secret"},"idempotency_key":"unrelated-replacement"}));
        assert_eq!(replaced.status, 200);
        let resumed = restart(&c);
        assert_eq!(
            in_runtime(|| resumed.connection(&id)).unwrap_err(),
            "onboarding_credentials_receipt_mismatch"
        );
        assert_eq!(
            in_runtime(|| resumed.api_keys(&id, paper_form().as_bytes())).unwrap_err(),
            "onboarding_credentials_receipt_mismatch"
        );
        attempt.actor = Some("different-owner".into());
        resumed.save(&attempt).unwrap();
        assert_eq!(
            in_runtime(|| resumed.connection(&id)).unwrap_err(),
            "onboarding_identity_changed"
        );
        assert!(!resumed.read(&id).unwrap().unwrap().api_keys_stored);
        let instance = service.handle_http_from_source(
            "fixture",
            "GET",
            "/plugins/instances/alpaca-paper",
            json!({}),
        );
        assert_eq!(instance.body["enabled"], false);
        assert_eq!(instance.body["credentialRevision"], 2);
    }

    #[test]
    fn paper_key_setup_preserves_preexisting_credentials_and_separate_relay_admission() {
        let (_dir, c) = fixture();
        let id = paper_attempt(&c);
        let mut attempt = c.read(&id).unwrap().unwrap();
        let service = in_runtime(|| c.bound_local_service(&attempt)).unwrap();
        let stored = service.handle_http_from_source("fixture", "POST", "/plugins/instances/alpaca-paper/credentials",
            json!({"credentials":{"api_key":"existing-key","api_secret":"existing-secret"},"idempotency_key":"preexisting"}));
        assert_eq!(stored.status, 200);
        assert_eq!(
            in_runtime(|| c.api_keys(&id, paper_form().as_bytes())).unwrap_err(),
            "onboarding_credentials_already_configured"
        );
        assert!(c.read(&id).unwrap().unwrap().api_key_intent.is_none());
        attempt.relay_requested = true;
        c.save(&attempt).unwrap();
        let status = in_runtime(|| c.public_status(&attempt)).unwrap();
        assert_eq!(status["ready"], false);
        assert_eq!(
            status["relay"],
            json!({"status":"unchecked","permitted":false})
        );
        // Relay selection does not prohibit local broker configuration.
        assert!(in_runtime(|| c.browser_action(&id, "paper-keys", vec![])).is_ok());
    }

    #[test]
    fn local_paper_key_customer_http_path_has_no_hosted_or_trade_effects() {
        let (_dir, c) = fixture();
        install_paper_manifest(&c);
        let started = start(&c);
        let url = started["browserUrl"].as_str().unwrap();
        let origin = url::Url::parse(url).unwrap().origin().ascii_serialization();
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let page = client.get(url).send().unwrap().text().unwrap();
        assert!(page.contains("Connect with local paper API keys"));
        let initial: Value = client
            .get(format!("{url}/connection"))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert!(initial["instance"].is_null(), "{initial}");
        let response = client
            .post(format!("{url}/paper-keys"))
            .header("origin", &origin)
            .send()
            .unwrap();
        let status = response.status();
        assert_eq!(status, 303, "{}", response.text().unwrap());
        let prepared: Value = client
            .get(format!("{url}/connection"))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(prepared["instance"]["accountMode"], "paper", "{prepared}");
        assert_eq!(prepared["instance"]["activePackageSha256"], "a".repeat(64));
        assert_ne!(prepared["instance"]["credentialStatus"]["configured"], true);
        let form = format!(
            "api_key=PAPER_KEY_CANARY&api_secret=PAPER_SECRET_CANARY&package_sha256={}",
            "a".repeat(64)
        );
        for invalid_origin in ["https://evil.test", "null"] {
            assert_eq!(
                client
                    .post(format!("{url}/api-keys"))
                    .header("origin", invalid_origin)
                    .body(form.clone())
                    .send()
                    .unwrap()
                    .status(),
                403
            );
        }
        let wrong = client
            .post(format!("{url}/api-keys"))
            .header("origin", &origin)
            .header("accept", "application/json")
            .body(form.replace(&"a".repeat(64), &"b".repeat(64)))
            .send()
            .unwrap();
        assert_eq!(wrong.status(), 400);
        let result = client
            .post(format!("{url}/api-keys"))
            .header("origin", &origin)
            .header("accept", "application/json")
            .body(form.clone())
            .send()
            .unwrap();
        assert_eq!(result.status(), 200);
        let text = result.text().unwrap();
        assert!(!text.contains("CANARY"));
        let stored: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(stored["credentialStatus"]["configured"], true, "{stored}");
        assert_eq!(
            stored["credentialStatus"]["authenticationMethod"],
            "api-key"
        );
        assert_eq!(stored["enabled"], true);
        assert_eq!(
            client
                .post(format!("{url}/api-keys"))
                .header("origin", &origin)
                .header("accept", "application/json")
                .body(form)
                .send()
                .unwrap()
                .status(),
            400
        );
        let retained: Value = client
            .get(format!("{url}/connection"))
            .send()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(retained["instance"], stored);
        assert!(crate::agent_runner::deployments(&c.service.runtime())
            .unwrap()
            .is_empty());
        let attempt = c
            .read(started["onboardingId"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert!(attempt.api_keys_stored);
        assert!(attempt.connection_id.is_none());
        assert!(!serde_json::to_string(&attempt).unwrap().contains("CANARY"));
    }

    #[test]
    fn paper_key_route_rejects_live_mode_and_ended_attempts() {
        let (_dir, c) = fixture();
        install_paper_manifest(&c);
        let started = start(&c);
        let id = started["onboardingId"].as_str().unwrap();
        let mut attempt = c.read(id).unwrap().unwrap();
        attempt.mode = "live".into();
        c.save(&attempt).unwrap();
        let result = tokio::runtime::Runtime::new().unwrap().block_on(async {
            tokio::task::block_in_place(|| c.browser_action(id, "paper-keys", vec![]))
        });
        assert_eq!(result.unwrap_err(), "onboarding_api_keys_paper_required");
        attempt.mode = "paper".into();
        attempt.cancelled = true;
        c.save(&attempt).unwrap();
        let result = tokio::runtime::Runtime::new().unwrap().block_on(async {
            tokio::task::block_in_place(|| c.browser_action(id, "paper-keys", vec![]))
        });
        assert_eq!(result.unwrap_err(), "onboarding_attempt_ended");
    }

    #[test]
    fn browser_page_is_reachable_and_cross_origin_mutation_is_denied() {
        let (_dir, c) = fixture();
        let result = start(&c);
        assert_eq!(result["status"], "browser_action_required", "{result}");
        assert_eq!(result["ready"], false);
        assert_eq!(result["identity"]["authenticated"], false);
        let url = result["browserUrl"].as_str().unwrap();
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let page = client.get(url).send().unwrap();
        assert_eq!(page.status(), 200);
        assert_eq!(page.headers()["cache-control"], "no-store");
        assert_eq!(page.headers()["referrer-policy"], "same-origin");
        let html = page.text().unwrap();
        assert!(html.contains("Connect TradeAssembly"));
        assert!(html.contains("Sign in to TradeAssembly"));
        assert!(!html.contains(">Connect broker</button>"));
        assert!(!html.contains("api_secret"));
        assert_eq!(
            client
                .get(url)
                .header("host", "evil.test")
                .send()
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .post(format!("{url}/cancel"))
                .header("origin", "https://evil.test")
                .send()
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .post(format!("{url}/cancel"))
                .send()
                .unwrap()
                .status(),
            403
        );
        let origin = url::Url::parse(url).unwrap().origin().ascii_serialization();
        assert_eq!(
            client
                .post(format!("{url}/cancel"))
                .header("origin", origin)
                .send()
                .unwrap()
                .status(),
            303
        );
        let status = invoke(
            &c,
            "tradeassembly.onboarding.status",
            json!({"onboardingId":result["onboardingId"]}),
        );
        assert_eq!(status["status"], "cancelled");
    }

    #[test]
    fn duplicate_start_is_stable_and_changed_mode_is_rejected() {
        let (_dir, c) = fixture();
        let first = start(&c);
        let second = start(&c);
        assert_eq!(first["onboardingId"], second["onboardingId"]);
        assert_eq!(first["browserUrl"], second["browserUrl"]);
        let conflict = invoke(
            &c,
            "tradeassembly.onboarding.start",
            json!({"mode":"live","environment":"staging","idempotency_key":"setup-1"}),
        );
        assert_eq!(conflict["error"]["code"], "onboarding_idempotency_conflict");
    }

    #[test]
    fn restart_reuses_persisted_attempt_and_renews_browser_capability() {
        let (_dir, c) = fixture();
        let first = start(&c);
        let mut restarted = c.clone();
        restarted.listener = Arc::new(Mutex::new(None));
        let status = invoke(
            &restarted,
            "tradeassembly.onboarding.status",
            json!({"onboardingId":first["onboardingId"]}),
        );
        assert_eq!(status["onboardingId"], first["onboardingId"]);
        assert_ne!(status["browserUrl"], first["browserUrl"]);
        assert_eq!(status["ready"], false);
    }

    #[test]
    fn expiry_and_cancellation_never_claim_readiness() {
        let (_dir, c) = fixture();
        let first = start(&c);
        let id = first["onboardingId"].as_str().unwrap();
        let mut record = c.read(id).unwrap().unwrap();
        record.expires_at_ms = 0;
        c.save(&record).unwrap();
        let status = invoke(
            &c,
            "tradeassembly.onboarding.status",
            json!({"onboardingId":id}),
        );
        assert_eq!(status["status"], "expired");
        assert_eq!(status["ready"], false);
        assert!(status["nextAction"]
            .as_str()
            .unwrap()
            .contains("new idempotency_key"));
        let cancelled = invoke(
            &c,
            "tradeassembly.onboarding.cancel",
            json!({"onboardingId":id}),
        );
        assert_eq!(cancelled["status"], "cancelled");
        assert_eq!(cancelled["ready"], false);
        assert_eq!(cancelled["nextAction"], status["nextAction"]);
    }
}
