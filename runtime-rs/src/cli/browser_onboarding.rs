//! Browser companion for the local MCP. No Studio process or source checkout.
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
                    json!({"instanceRef":instance,"mode":attempt.mode,"idempotency_key":format!("{}:verify",attempt.id)}),
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
        if progress["hubAuthenticated"] != true
            || (attempt.connection_id.is_some() && progress["ready"] != true)
        {
            links.push_str("<meta http-equiv=refresh content=5>");
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
        if let Some(instance) = attempt
            .instance_ref
            .as_ref()
            .filter(|_| attempt.connection_id.is_none())
        {
            let (service, _) = self.bound_service(&attempt)?;
            let descriptor = service.broker_connection_descriptor(instance)?;
            links.push_str(&format!("<form method=post target=_blank action=\"{id}/authorize\"><fieldset><legend>Broker permissions</legend>"));
            for scope in descriptor["scopes"].as_array().into_iter().flatten() {
                if let (Some(key), Some(label)) = (scope["id"].as_str(), scope["label"].as_str()) {
                    links.push_str(&format!(
                        "<p><label><input type=checkbox name=scope value=\"{}\">{}</label></p>",
                        escape(key),
                        escape(label)
                    ));
                }
            }
            links.push_str("</fieldset><button>Continue to broker authorization</button></form>");
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
        if progress["hubAuthenticated"] != true {
            links.push_str(&format!("<form method=post target=_blank action=\"{id}/login\"><button>Sign in to TradeAssembly</button></form>"));
        } else if attempt.instance_ref.is_none() {
            links.push_str(&format!(
                "<form method=post action=\"{id}/broker\"><button>Connect broker</button></form>"
            ));
        }
        let controls=format!("<p>Broker account: {}. Deployment: {}.</p>{links}<form method=post action=\"{id}/cancel\"><button>Cancel setup</button></form>",escape(&attempt.mode),escape(&attempt.environment));
        Ok(document(
            if progress["ready"] == true {
                "Connection verified. You can return to your agent."
            } else {
                match progress["status"].as_str() {
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
            "broker" | "authorize" => {
                if attempt.connection_id.is_some() {
                    return Ok(attempt.authorization_url.clone());
                }
                let (service, identity) = self.bound_service(&attempt)?;
                let instance = if let Some(instance) = attempt.instance_ref.as_ref() {
                    // Require ownership without replaying prepare with changed
                    // inputs under the original command idempotency key.
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
    format!("<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>Connect TradeAssembly</title><h1>Connect TradeAssembly</h1><p>{}</p>{controls}<p>Credentials stay in the sign-in providers. Never paste keys into agent chat.</p></html>",escape(message))
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
    use super::*;

    fn fixture() -> (tempfile::TempDir, BrowserOnboarding) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("runtime.db").display().to_string();
        let service = TradeAssemblyService::test_local(db.clone());
        let mut config = crate::runtime_config::RuntimeConfig::local(db);
        config.oidc_profile = "local_owner".into();
        config.oidc_session_path = dir.path().join("session.bin").display().to_string();
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
