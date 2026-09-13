// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

const LOCAL_STABLE_IDENTITY_ID: &str = "local:user_123";
const LOCAL_INSTALL_ID: &str = "install_local_123";
const LOCAL_ACCOUNT_URL: &str = "/account";
const LOCAL_INSTALL_URL: &str = "/account/install";
const LOCAL_TELEMETRY_URL: &str = "/account/telemetry";
const LOCAL_TELEMETRY_DEBUG_URL: &str = "/telemetry/debug";

const LOCAL_OWNER_ALLOW: &[&str] = &[
    "strategy.*",
    "strategy_version.*",
    "strategy_analysis.*",
    "strategy_contract.*",
    "strategy_execution_config.*",
    "strategy_execution_activation.*",
    "provider_account.*",
    "plugin_instance.*",
    "plugin_manifest_local.*",
    "credential.*",
    "credential_metadata.*",
    "providers.credentials.*",
    "research.*",
    "backtest.*",
    "scheduler.*",
    "orders.*",
    "risk.*",
    "positions.*",
    "journal.*",
    "replay.*",
    "share.*",
    "alert.*",
    "monitoring.read.*",
];

const COMMERCIAL_DENY: &[&str] = &[
    "billing.*",
    "checkout.*",
    "purchase.*",
    "payout.*",
    "creator_payout.*",
    "hosted_oauth.*",
    "managed_worker.*",
    "enterprise_sso.*",
    "enterprise_rbac.*",
    "operator_dashboard.*",
    "break_glass.*",
    "platform.operations.*",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthzActor {
    pub principal_id: String,
    pub roles: Vec<String>,
    pub plan_id: String,
    pub account_id: String,
    pub explicit_capabilities: Vec<String>,
}

impl Default for AuthzActor {
    fn default() -> Self {
        Self {
            principal_id: "local-owner".to_string(),
            roles: vec!["local_owner".to_string()],
            plan_id: "local_full".to_string(),
            account_id: "local".to_string(),
            explicit_capabilities: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AuthzTarget {
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub account_id: Option<String>,
    pub owner_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthzRequest {
    pub capability: String,
    pub actor: AuthzActor,
    pub target: Option<AuthzTarget>,
    pub attributes: Map<String, Value>,
}

impl Default for AuthzRequest {
    fn default() -> Self {
        Self {
            capability: String::new(),
            actor: AuthzActor::default(),
            target: None,
            attributes: Map::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AuthzDecision {
    pub allowed: bool,
    pub reason: String,
    pub engine: String,
    pub evidence: Map<String, Value>,
}

pub fn authorize_local_owner(request: AuthzRequest) -> AuthzDecision {
    let capability = request.capability.trim();
    if capability.is_empty() {
        return decision(false, "capability_required", evidence(&request.actor, None));
    }
    if matches_any(capability, COMMERCIAL_DENY) {
        return decision(
            false,
            "commercial_capability_denied",
            evidence(&request.actor, Some(("denied_by", capability))),
        );
    }
    if matches_any_owned(capability, &request.actor.explicit_capabilities)
        || matches_any(capability, LOCAL_OWNER_ALLOW)
    {
        return decision(true, "allowed_local_owner", evidence(&request.actor, None));
    }
    decision(
        false,
        "capability_not_declared",
        evidence(&request.actor, None),
    )
}

pub fn local_owner_workspace_entitlements(capabilities: &[String]) -> Value {
    let selected = if capabilities.is_empty() {
        vec![
            "strategy.create".to_string(),
            "strategy.update".to_string(),
            "strategy_version.publish".to_string(),
            "strategy_execution_config.create".to_string(),
            "strategy_execution_activation.activate.paper".to_string(),
            "strategy_execution_activation.activate.live".to_string(),
            "strategy_execution_activation.deactivate".to_string(),
            "research.backtest.run".to_string(),
            "providers.credentials.local_store".to_string(),
            "credential.create".to_string(),
            "scheduler.start".to_string(),
            "orders.reconcile".to_string(),
            "risk.status".to_string(),
            "journal.replay".to_string(),
            "share.export".to_string(),
        ]
    } else {
        capabilities.to_vec()
    };
    let mut status = Map::new();
    for capability in selected {
        status.insert(
            capability.clone(),
            Value::Bool(
                authorize_local_owner(AuthzRequest {
                    capability,
                    ..AuthzRequest::default()
                })
                .allowed,
            ),
        );
    }
    let mut payload = Map::new();
    payload.insert(
        "account".to_string(),
        Value::String("local-owner".to_string()),
    );
    payload.insert("role".to_string(), Value::String("local_owner".to_string()));
    payload.insert("plan".to_string(), Value::String("local_full".to_string()));
    payload.insert(
        "commercialCapabilitiesDenied".to_string(),
        Value::Bool(true),
    );
    payload.extend(status.clone());
    payload.insert("capabilities".to_string(), Value::Object(status));
    Value::Object(payload)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanLimits {
    pub max_active_activations: u32,
    pub evaluation_frequency_tier: String,
    pub journal_retention_days: u32,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            max_active_activations: 1000,
            evaluation_frequency_tier: "high".to_string(),
            journal_retention_days: 3650,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountPlan {
    pub id: String,
    pub limits: PlanLimits,
    pub grants: Vec<String>,
}

impl Default for AccountPlan {
    fn default() -> Self {
        Self {
            id: "local_full".to_string(),
            limits: PlanLimits::default(),
            grants: vec!["local_owner".to_string()],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivationPack {
    pub id: String,
    pub limits: PlanLimits,
    pub grants: Vec<String>,
}

pub fn compute_limits(plan: &AccountPlan, packs: &[ActivationPack]) -> PlanLimits {
    let mut limits = plan.limits.clone();
    for pack in packs {
        limits.max_active_activations = limits
            .max_active_activations
            .max(pack.limits.max_active_activations);
        limits.evaluation_frequency_tier = higher_frequency_tier(
            &limits.evaluation_frequency_tier,
            &pack.limits.evaluation_frequency_tier,
        );
        limits.journal_retention_days = limits
            .journal_retention_days
            .max(pack.limits.journal_retention_days);
    }
    limits
}

pub fn compute_grants(plan: &AccountPlan, packs: &[ActivationPack]) -> Vec<String> {
    let mut grants = BTreeSet::from_iter(plan.grants.iter().cloned());
    for pack in packs {
        grants.extend(pack.grants.iter().cloned());
    }
    grants.into_iter().collect()
}

pub fn effective_entitlements(plan: Option<AccountPlan>, packs: &[ActivationPack]) -> Value {
    let resolved_plan = plan.unwrap_or_default();
    let mut payload = Map::new();
    payload.insert("plan".to_string(), Value::String(resolved_plan.id.clone()));
    payload.insert(
        "limits".to_string(),
        serde_json::to_value(compute_limits(&resolved_plan, packs)).expect("serialize limits"),
    );
    payload.insert(
        "grants".to_string(),
        Value::Array(
            compute_grants(&resolved_plan, packs)
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
    );
    payload.insert(
        "workspace".to_string(),
        local_owner_workspace_entitlements(&[]),
    );
    Value::Object(payload)
}

pub fn local_account_status(profile: &str, studio_base_url: &str) -> Value {
    json!({
        "schemaVersion": "tradeassembly.account.status.v1",
        "ok": true,
        "profile": normalize_profile(profile),
        "mode": "local",
        "authenticated": true,
        "stable_identity_id": LOCAL_STABLE_IDENTITY_ID,
        "install_id": LOCAL_INSTALL_ID,
        "install_status": "claimed",
        "claim_status": "claimed",
        "entitlements": ["solo", "paper"],
        "telemetry_opt_out": false,
        "credential_custody": "local_or_customer_managed",
        "no_credential_custody": true,
        "deep_links": {
            "account": deep_link(studio_base_url, LOCAL_ACCOUNT_URL),
            "install": deep_link(studio_base_url, LOCAL_INSTALL_URL),
            "telemetry": deep_link(studio_base_url, LOCAL_TELEMETRY_URL)
        },
        "safe_next_action": "Use local account, install, and telemetry routes for development; hosted identity is not required."
    })
}

pub fn local_account_login(profile: &str, studio_base_url: &str) -> Value {
    json!({
        "schemaVersion": "tradeassembly.account.login.v1",
        "ok": true,
        "profile": normalize_profile(profile),
        "mode": "local",
        "provider": "local",
        "subject": "user_123",
        "stable_identity_id": LOCAL_STABLE_IDENTITY_ID,
        "login_url": deep_link(studio_base_url, "/auth/login?return_to=/account"),
        "return_url": deep_link(studio_base_url, LOCAL_ACCOUNT_URL),
        "credential_custody": "local_or_customer_managed",
        "no_credential_custody": true
    })
}

pub fn local_install_claim(
    profile: &str,
    install_id: Option<&str>,
    studio_base_url: &str,
) -> Value {
    let install_id = install_id.unwrap_or(LOCAL_INSTALL_ID);
    json!({
        "schemaVersion": "tradeassembly.install.claim.v1",
        "ok": true,
        "profile": normalize_profile(profile),
        "stable_identity_id": LOCAL_STABLE_IDENTITY_ID,
        "install_id": install_id,
        "status": "claimed",
        "claim_status": "claimed",
        "claimed_at": "2026-06-28T18:00:00Z",
        "idempotency_key": format!("claim-{install_id}-{LOCAL_STABLE_IDENTITY_ID}"),
        "deep_link": deep_link(studio_base_url, LOCAL_INSTALL_URL),
        "no_credential_custody": true
    })
}

pub fn local_telemetry_emit(
    profile: &str,
    event_type: &str,
    install_id: Option<&str>,
    studio_base_url: &str,
) -> Value {
    let install_id = install_id.unwrap_or(LOCAL_INSTALL_ID);
    json!({
        "schemaVersion": "tradeassembly.telemetry.emit.v1",
        "ok": true,
        "profile": normalize_profile(profile),
        "status": "accepted",
        "event_type": event_type,
        "install_id": install_id,
        "sink": ".local/telemetry/events.jsonl",
        "payload": {
            "surface": "local",
            "schema_version": 1
        },
        "deep_links": {
            "telemetry": deep_link(studio_base_url, LOCAL_TELEMETRY_URL),
            "debug": deep_link(studio_base_url, LOCAL_TELEMETRY_DEBUG_URL)
        },
        "no_credential_custody": true
    })
}

pub fn local_telemetry_opt_out(profile: &str, opted_out: bool, studio_base_url: &str) -> Value {
    json!({
        "schemaVersion": "tradeassembly.telemetry.opt_out.v1",
        "ok": true,
        "profile": normalize_profile(profile),
        "stable_identity_id": LOCAL_STABLE_IDENTITY_ID,
        "install_id": LOCAL_INSTALL_ID,
        "telemetry_opt_out": opted_out,
        "future_event_status": if opted_out { "skipped" } else { "accepted" },
        "deep_link": deep_link(studio_base_url, LOCAL_TELEMETRY_URL),
        "no_credential_custody": true
    })
}

pub fn local_telemetry_replay(profile: &str, studio_base_url: &str) -> Value {
    json!({
        "schemaVersion": "tradeassembly.telemetry.replay.v1",
        "ok": true,
        "profile": normalize_profile(profile),
        "sink": ".local/telemetry/events.jsonl",
        "events": [],
        "replay_command": "tradeassembly telemetry replay --profile local",
        "deep_link": deep_link(studio_base_url, LOCAL_TELEMETRY_DEBUG_URL),
        "no_credential_custody": true
    })
}

fn normalize_profile(profile: &str) -> String {
    if profile.trim().is_empty() {
        "local".to_string()
    } else {
        profile.trim().to_string()
    }
}

fn deep_link(studio_base_url: &str, path: &str) -> String {
    format!("{}{}", studio_base_url.trim_end_matches('/'), path)
}

pub fn capability_status(capabilities: &[String]) -> Map<String, Value> {
    let mut status = Map::new();
    for capability in capabilities {
        status.insert(
            capability.clone(),
            Value::Bool(
                authorize_local_owner(AuthzRequest {
                    capability: capability.clone(),
                    ..AuthzRequest::default()
                })
                .allowed,
            ),
        );
    }
    status
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionValidationRequest {
    pub provider: String,
    pub session_token: Option<String>,
    pub access_token: Option<String>,
    pub id_token: Option<String>,
    pub metadata: Value,
}

impl Default for SessionValidationRequest {
    fn default() -> Self {
        Self {
            provider: "local".to_string(),
            session_token: None,
            access_token: None,
            id_token: None,
            metadata: Value::Object(Map::new()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionPrincipal {
    pub provider: String,
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionValidationResult {
    pub authenticated: bool,
    pub reason: String,
    pub principal: Option<SessionPrincipal>,
}

pub fn local_session_validate(request: SessionValidationRequest) -> SessionValidationResult {
    let subject = raw_string_field(&request.metadata, "subject")
        .unwrap_or_else(|| "local-owner".to_string())
        .trim()
        .to_string();
    if subject.is_empty() {
        return SessionValidationResult {
            authenticated: false,
            reason: "subject_missing".to_string(),
            principal: None,
        };
    }
    SessionValidationResult {
        authenticated: true,
        reason: "ok".to_string(),
        principal: Some(SessionPrincipal {
            provider: "local".to_string(),
            issuer: "local".to_string(),
            subject,
            email: string_field(&request.metadata, "email"),
            email_verified: bool_field(&request.metadata, "email_verified"),
            name: string_field(&request.metadata, "name")
                .or_else(|| Some("Local Owner".to_string())),
            picture: string_field(&request.metadata, "picture"),
        }),
    }
}

pub fn validate_session(
    request: SessionValidationRequest,
    kratos_public_url: Option<&str>,
    cognito_userinfo_url: Option<&str>,
    response: Option<IdentityProviderResponse>,
) -> SessionValidationResult {
    match request.provider.as_str() {
        "local" => local_session_validate(request),
        "kratos" => kratos_session_validate(request, kratos_public_url, response),
        "cognito" => cognito_session_validate(request, cognito_userinfo_url, response),
        _ => SessionValidationResult {
            authenticated: false,
            reason: "unsupported_provider".to_string(),
            principal: None,
        },
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdentityProviderResponse {
    pub status_code: u16,
    pub payload: Value,
}

pub fn kratos_session_validate(
    request: SessionValidationRequest,
    public_url: Option<&str>,
    response: Option<IdentityProviderResponse>,
) -> SessionValidationResult {
    if request
        .session_token
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return unauthenticated("missing_session_token");
    }
    if public_url.unwrap_or("").trim().is_empty() {
        return unauthenticated("validator_url_missing");
    }
    let Some(response) = response else {
        return unauthenticated("validator_unavailable");
    };
    if response.status_code != 200 {
        return unauthenticated("invalid_session");
    }
    let Some(identity) = response.payload.get("identity").and_then(Value::as_object) else {
        return unauthenticated("subject_missing");
    };
    let subject = identity
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if subject.is_empty() {
        return unauthenticated("subject_missing");
    }
    let traits = identity
        .get("traits")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    SessionValidationResult {
        authenticated: true,
        reason: "ok".to_string(),
        principal: Some(SessionPrincipal {
            provider: "kratos".to_string(),
            issuer: "kratos".to_string(),
            subject,
            email: string_field(&traits, "email"),
            email_verified: bool_field(&traits, "email_verified"),
            name: string_field(&traits, "name"),
            picture: string_field(&traits, "picture"),
        }),
    }
}

pub fn cognito_session_validate(
    request: SessionValidationRequest,
    userinfo_url: Option<&str>,
    response: Option<IdentityProviderResponse>,
) -> SessionValidationResult {
    if request
        .access_token
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return unauthenticated("missing_access_token");
    }
    if userinfo_url.unwrap_or("").trim().is_empty() {
        return unauthenticated("userinfo_url_missing");
    }
    let Some(response) = response else {
        return unauthenticated("validator_unavailable");
    };
    if response.status_code != 200 {
        return unauthenticated("invalid_access_token");
    }
    let subject = response
        .payload
        .get("sub")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if subject.is_empty() {
        return unauthenticated("subject_missing");
    }
    SessionValidationResult {
        authenticated: true,
        reason: "ok".to_string(),
        principal: Some(SessionPrincipal {
            provider: "cognito".to_string(),
            issuer: "cognito".to_string(),
            subject,
            email: string_field(&response.payload, "email"),
            email_verified: bool_field(&response.payload, "email_verified"),
            name: string_field(&response.payload, "name"),
            picture: string_field(&response.payload, "picture"),
        }),
    }
}

pub fn local_issuer_discovery(issuer: Option<&str>) -> Value {
    let issuer = issuer.unwrap_or("local-owner").trim();
    if issuer == "local" || issuer == "local-owner" || issuer.is_empty() {
        json_object([
            ("available", Value::Bool(true)),
            ("reason", Value::String("ok".to_string())),
            ("issuer", Value::String("local-owner".to_string())),
            ("authorization_endpoint", Value::Null),
            (
                "supported_algorithms",
                Value::Array(vec![Value::String("none".to_string())]),
            ),
        ])
    } else {
        json_object([
            ("available", Value::Bool(false)),
            ("reason", Value::String("issuer_unavailable".to_string())),
            ("issuer", Value::String(issuer.to_string())),
        ])
    }
}

pub fn local_login_start(requested_scopes: &[String]) -> Value {
    json_object([
        ("configured", Value::Bool(false)),
        ("reason", Value::String("login_not_configured".to_string())),
        (
            "instructions",
            Value::String("Local-owner mode is active; no identity login is required.".to_string()),
        ),
        (
            "requested_scopes",
            Value::Array(
                requested_scopes
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        ),
    ])
}

pub fn local_identity_status(
    install_id: &str,
    credential_custody_mode: &str,
    metrics_opt_out: bool,
    session: SessionValidationRequest,
) -> Value {
    let validation = validate_session(session, None, None, None);
    json_object([
        ("mode", Value::String("local".to_string())),
        ("authenticated", Value::Bool(validation.authenticated)),
        ("reason", Value::String(validation.reason)),
        (
            "principal",
            serde_json::to_value(validation.principal).unwrap_or(Value::Null),
        ),
        ("issuer", Value::String("local-owner".to_string())),
        ("install_id", Value::String(install_id.to_string())),
        (
            "install_claim_status",
            Value::String("not_required".to_string()),
        ),
        ("metrics_opt_out", Value::Bool(metrics_opt_out)),
        (
            "credential_custody_mode",
            Value::String(credential_custody_mode.to_string()),
        ),
        (
            "next_action",
            Value::String(if validation.authenticated {
                "continue_local".to_string()
            } else {
                "repair_identity".to_string()
            }),
        ),
    ])
}

pub fn local_logout(provider: &str) -> Value {
    if provider == "local" {
        json_object([
            ("logged_out", Value::Bool(true)),
            ("reason", Value::String("not_applicable".to_string())),
        ])
    } else {
        json_object([
            ("logged_out", Value::Bool(true)),
            ("reason", Value::String("local_state_cleared".to_string())),
            (
                "warning",
                Value::String("remote_logout_not_configured".to_string()),
            ),
        ])
    }
}

pub fn metrics_opt_out_state(
    current_opted_out: bool,
    requested: Option<bool>,
    reason: Option<&str>,
) -> Value {
    json_object([
        (
            "opted_out",
            Value::Bool(requested.unwrap_or(current_opted_out)),
        ),
        (
            "reason",
            Value::String(reason.unwrap_or("status").to_string()),
        ),
        ("source", Value::String("local".to_string())),
    ])
}

pub fn verify_install_claim(raw_claim: Value, expected_install_id: &str) -> Value {
    let Some(claim) = raw_claim.as_object() else {
        return install_claim_result(false, "missing_claim", Value::Null);
    };
    if claim.is_empty() {
        return install_claim_result(false, "missing_claim", Value::Null);
    }
    if let Some(reason) = forbidden_claim_reason(&raw_claim) {
        return install_claim_result(false, reason, Value::Null);
    }
    let install_id = claim
        .get("install_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if install_id != expected_install_id {
        return install_claim_result(false, "install_id_mismatch", Value::Null);
    }
    let issuer = claim
        .get("issuer")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if issuer != "local-owner" {
        return install_claim_result(false, "claim_verifier_not_configured", Value::Null);
    }
    let subject = claim
        .get("subject")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let organization_id = claim
        .get("organization_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if subject.is_empty() && organization_id.is_empty() {
        return install_claim_result(false, "subject_missing", Value::Null);
    }
    install_claim_result(true, "ok", raw_claim)
}

fn decision(allowed: bool, reason: &str, evidence: Map<String, Value>) -> AuthzDecision {
    AuthzDecision {
        allowed,
        reason: reason.to_string(),
        engine: "local_owner".to_string(),
        evidence,
    }
}

fn unauthenticated(reason: &str) -> SessionValidationResult {
    SessionValidationResult {
        authenticated: false,
        reason: reason.to_string(),
        principal: None,
    }
}

fn json_object<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Object(Map::from_iter(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value)),
    ))
}

fn install_claim_result(valid: bool, reason: &str, claim: Value) -> Value {
    json_object([
        ("valid", Value::Bool(valid)),
        ("reason", Value::String(reason.to_string())),
        ("claim", claim),
    ])
}

fn forbidden_claim_reason(value: &Value) -> Option<&'static str> {
    const CREDENTIAL_KEYS: &[&str] = &[
        "access_token",
        "api_key",
        "api_secret",
        "broker_credentials",
        "credential",
        "credentials",
        "data_credentials",
        "refresh_token",
    ];
    const SENSITIVE_KEYS: &[&str] = &[
        "account_balance",
        "account_balances",
        "journal",
        "positions",
        "strategy_secret",
        "strategy_secrets",
        "strategy_source",
        "trading_strategy_secrets",
    ];

    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let normalized = key.trim().to_ascii_lowercase();
                if CREDENTIAL_KEYS.contains(&normalized.as_str()) {
                    return Some("credential_custody_forbidden");
                }
                if SENSITIVE_KEYS.contains(&normalized.as_str()) {
                    return Some("sensitive_claim_forbidden");
                }
                if let Some(reason) = forbidden_claim_reason(child) {
                    return Some(reason);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(forbidden_claim_reason),
        _ => None,
    }
}

fn evidence(actor: &AuthzActor, extra: Option<(&str, &str)>) -> Map<String, Value> {
    let mut evidence = Map::from_iter([
        (
            "principal_id".to_string(),
            Value::String(actor.principal_id.clone()),
        ),
        (
            "roles".to_string(),
            Value::Array(actor.roles.iter().cloned().map(Value::String).collect()),
        ),
        ("plan_id".to_string(), Value::String(actor.plan_id.clone())),
        (
            "account_id".to_string(),
            Value::String(actor.account_id.clone()),
        ),
        (
            "policy".to_string(),
            Value::String("local_owner_full_non_commercial".to_string()),
        ),
    ]);
    if let Some((key, value)) = extra {
        evidence.insert(key.to_string(), Value::String(value.to_string()));
    }
    evidence
}

fn matches_any(capability: &str, patterns: &[&str]) -> bool {
    patterns
        .iter()
        .any(|pattern| wildcard_matches(capability, pattern))
}

fn matches_any_owned(capability: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| wildcard_matches(capability, pattern))
}

fn wildcard_matches(value: &str, pattern: &str) -> bool {
    if pattern == "*" || pattern == value {
        return true;
    }
    let Some(first_star) = pattern.find('*') else {
        return false;
    };
    let prefix = &pattern[..first_star];
    let suffix = &pattern[first_star + 1..];
    if !value.starts_with(prefix) || !value.ends_with(suffix) {
        return false;
    }
    if pattern.matches('*').count() == 1 {
        return value.len() >= prefix.len() + suffix.len();
    }
    let mut remainder = &value[prefix.len()..value.len() - suffix.len()];
    for part in pattern[first_star + 1..pattern.len() - suffix.len()].split('*') {
        if part.is_empty() {
            continue;
        }
        let Some(index) = remainder.find(part) else {
            return false;
        };
        remainder = &remainder[index + part.len()..];
    }
    true
}

fn higher_frequency_tier(left: &str, right: &str) -> String {
    if frequency_tier_rank(left) >= frequency_tier_rank(right) {
        left.to_string()
    } else {
        right.to_string()
    }
}

fn frequency_tier_rank(value: &str) -> u8 {
    match value {
        "medium" => 1,
        "high" => 2,
        _ => 0,
    }
}

fn string_field(payload: &Value, key: &str) -> Option<String> {
    let text = raw_string_field(payload, key)?.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn raw_string_field(payload: &Value, key: &str) -> Option<String> {
    payload.as_object()?.get(key)?.as_str().map(str::to_string)
}

fn bool_field(payload: &Value, key: &str) -> Option<bool> {
    match payload.as_object()?.get(key)? {
        Value::Bool(value) => Some(*value),
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        authorize_local_owner, capability_status, compute_grants, compute_limits,
        effective_entitlements, kratos_session_validate, local_session_validate, validate_session,
        verify_install_claim, AccountPlan, ActivationPack, AuthzActor, AuthzRequest,
        IdentityProviderResponse, PlanLimits, SessionValidationRequest,
    };
    use serde_json::{json, Value};

    #[test]
    fn local_owner_authz_allows_non_commercial_and_denies_hosted_surfaces() {
        let allowed = authorize_local_owner(AuthzRequest {
            capability: "strategy.create".to_string(),
            ..AuthzRequest::default()
        });
        let denied = authorize_local_owner(AuthzRequest {
            capability: "billing.checkout".to_string(),
            ..AuthzRequest::default()
        });
        let explicit = authorize_local_owner(AuthzRequest {
            capability: "custom.capability".to_string(),
            actor: AuthzActor {
                explicit_capabilities: vec!["custom.*".to_string()],
                ..AuthzActor::default()
            },
            ..AuthzRequest::default()
        });

        assert!(allowed.allowed);
        assert_eq!(allowed.reason, "allowed_local_owner");
        assert_eq!(allowed.evidence["principal_id"], "local-owner");
        assert!(!denied.allowed);
        assert_eq!(denied.reason, "commercial_capability_denied");
        assert_eq!(denied.evidence["denied_by"], "billing.checkout");
        assert!(explicit.allowed);
    }

    #[test]
    fn local_owner_authz_requires_a_capability() {
        let decision = authorize_local_owner(AuthzRequest {
            capability: "   ".to_string(),
            ..AuthzRequest::default()
        });

        assert!(!decision.allowed);
        assert_eq!(decision.reason, "capability_required");
    }

    #[test]
    fn local_session_validation_preserves_source_shaped_metadata() {
        let result = local_session_validate(SessionValidationRequest {
            metadata: json!({
                "subject": " owner-1 ",
                "email": "owner@example.test",
                "email_verified": "true",
                "name": "Owner",
                "picture": "https://example.test/avatar.png"
            }),
            ..SessionValidationRequest::default()
        });

        assert!(result.authenticated);
        assert_eq!(result.reason, "ok");
        let principal = result.principal.expect("principal");
        assert_eq!(principal.provider, "local");
        assert_eq!(principal.subject, "owner-1");
        assert_eq!(principal.email.as_deref(), Some("owner@example.test"));
        assert_eq!(principal.email_verified, Some(true));
        assert_eq!(principal.name.as_deref(), Some("Owner"));
    }

    #[test]
    fn local_session_validation_fails_closed_when_subject_is_blank() {
        let result = local_session_validate(SessionValidationRequest {
            metadata: json!({"subject": "   "}),
            ..SessionValidationRequest::default()
        });

        assert!(!result.authenticated);
        assert_eq!(result.reason, "subject_missing");
        assert!(result.principal.is_none());
    }

    #[test]
    fn delegated_session_validation_routes_local_and_fails_unknown() {
        let local = validate_session(SessionValidationRequest::default(), None, None, None);
        let unknown = validate_session(
            SessionValidationRequest {
                provider: "github".to_string(),
                ..SessionValidationRequest::default()
            },
            None,
            None,
            None,
        );

        assert!(local.authenticated);
        assert!(!unknown.authenticated);
        assert_eq!(unknown.reason, "unsupported_provider");
    }

    #[test]
    fn kratos_validator_fails_closed_without_url_or_token() {
        let missing_token = kratos_session_validate(
            SessionValidationRequest {
                provider: "kratos".to_string(),
                ..SessionValidationRequest::default()
            },
            Some("https://kratos.example"),
            None,
        );
        let missing_url = kratos_session_validate(
            SessionValidationRequest {
                provider: "kratos".to_string(),
                session_token: Some("token".to_string()),
                ..SessionValidationRequest::default()
            },
            None,
            None,
        );

        assert_eq!(missing_token.reason, "missing_session_token");
        assert_eq!(missing_url.reason, "validator_url_missing");
    }

    #[test]
    fn kratos_validator_accepts_self_hosted_whoami_response() {
        let result = validate_session(
            SessionValidationRequest {
                provider: "kratos".to_string(),
                session_token: Some("session-token".to_string()),
                ..SessionValidationRequest::default()
            },
            Some("https://kratos.example"),
            None,
            Some(IdentityProviderResponse {
                status_code: 200,
                payload: json!({
                    "identity": {
                        "id": "subject-1",
                        "traits": {
                            "email": "user@example.test",
                            "email_verified": "true",
                            "name": "User"
                        }
                    }
                }),
            }),
        );

        assert!(result.authenticated);
        let principal = result.principal.expect("principal");
        assert_eq!(principal.provider, "kratos");
        assert_eq!(principal.subject, "subject-1");
        assert_eq!(principal.email.as_deref(), Some("user@example.test"));
        assert_eq!(principal.email_verified, Some(true));
    }

    #[test]
    fn cognito_validator_accepts_self_hosted_userinfo_response() {
        let result = validate_session(
            SessionValidationRequest {
                provider: "cognito".to_string(),
                access_token: Some("access-token".to_string()),
                ..SessionValidationRequest::default()
            },
            None,
            Some("https://cognito.example/oauth2/userInfo"),
            Some(IdentityProviderResponse {
                status_code: 200,
                payload: json!({
                    "sub": "subject-2",
                    "email": "cognito@example.test",
                    "email_verified": false
                }),
            }),
        );

        assert!(result.authenticated);
        let principal = result.principal.expect("principal");
        assert_eq!(principal.provider, "cognito");
        assert_eq!(principal.subject, "subject-2");
        assert_eq!(principal.email.as_deref(), Some("cognito@example.test"));
        assert_eq!(principal.email_verified, Some(false));
    }

    #[test]
    fn local_identity_client_exposes_offline_identity_contract() {
        let discovery = super::local_issuer_discovery(None);
        let login = super::local_login_start(&[]);
        let session = validate_session(SessionValidationRequest::default(), None, None, None);
        let status = super::local_identity_status(
            "install-local",
            "local-file",
            false,
            SessionValidationRequest::default(),
        );
        let logout = super::local_logout("local");

        assert_eq!(discovery["available"], true);
        assert_eq!(discovery["issuer"], "local-owner");
        assert_eq!(discovery["authorization_endpoint"], Value::Null);
        assert_eq!(login["configured"], false);
        assert_eq!(login["reason"], "login_not_configured");
        assert!(session.authenticated);
        assert_eq!(status["mode"], "local");
        assert_eq!(status["authenticated"], true);
        assert_eq!(status["install_id"], "install-local");
        assert_eq!(status["credential_custody_mode"], "local-file");
        assert_eq!(status["next_action"], "continue_local");
        assert_eq!(logout["logged_out"], true);
        assert_eq!(logout["reason"], "not_applicable");
    }

    #[test]
    fn identity_client_persists_local_metrics_opt_out_state_shape() {
        let initial = super::metrics_opt_out_state(false, None, None);
        let updated = super::metrics_opt_out_state(false, Some(true), Some("user_request"));
        let status = super::local_identity_status(
            "local-install",
            "local",
            updated["opted_out"].as_bool().unwrap_or(false),
            SessionValidationRequest::default(),
        );

        assert_eq!(initial["opted_out"], false);
        assert_eq!(updated["opted_out"], true);
        assert_eq!(updated["reason"], "user_request");
        assert_eq!(status["metrics_opt_out"], true);
    }

    #[test]
    fn identity_client_rejects_sensitive_install_claim_fields() {
        let result = verify_install_claim(
            json!({
                "install_id": "install-local",
                "issuer": "local-owner",
                "subject": "local-owner",
                "broker_credentials": {"api_key": "redacted"}
            }),
            "install-local",
        );

        assert_eq!(result["valid"], false);
        assert_eq!(result["reason"], "credential_custody_forbidden");
    }

    #[test]
    fn identity_client_verifies_local_unsigned_claim_but_not_hosted_claim() {
        let local = verify_install_claim(
            json!({
                "install_id": "install-local",
                "issuer": "local-owner",
                "subject": "local-owner"
            }),
            "install-local",
        );
        let hosted = verify_install_claim(
            json!({
                "install_id": "install-local",
                "issuer": "tradeassembly-org-idp",
                "subject": "user-1"
            }),
            "install-local",
        );

        assert_eq!(local["valid"], true);
        assert_eq!(local["claim"]["issuer"], "local-owner");
        assert_eq!(hosted["valid"], false);
        assert_eq!(hosted["reason"], "claim_verifier_not_configured");
    }

    #[test]
    fn entitlement_merge_preserves_source_shaped_limits_and_grants() {
        let plan = AccountPlan {
            id: "local".to_string(),
            limits: PlanLimits {
                max_active_activations: 2,
                evaluation_frequency_tier: "low".to_string(),
                journal_retention_days: 30,
            },
            grants: vec!["strategy.create".to_string()],
        };
        let packs = vec![
            ActivationPack {
                id: "research".to_string(),
                limits: PlanLimits {
                    max_active_activations: 10,
                    evaluation_frequency_tier: "medium".to_string(),
                    journal_retention_days: 365,
                },
                grants: vec!["research.backtest.run".to_string()],
            },
            ActivationPack {
                id: "runtime".to_string(),
                limits: PlanLimits {
                    max_active_activations: 5,
                    evaluation_frequency_tier: "high".to_string(),
                    journal_retention_days: 90,
                },
                grants: vec!["orders.submit".to_string()],
            },
        ];

        let limits = compute_limits(&plan, &packs);
        let grants = compute_grants(&plan, &packs);

        assert_eq!(limits.max_active_activations, 10);
        assert_eq!(limits.evaluation_frequency_tier, "high");
        assert_eq!(limits.journal_retention_days, 365);
        assert_eq!(
            grants,
            vec![
                "orders.submit".to_string(),
                "research.backtest.run".to_string(),
                "strategy.create".to_string()
            ]
        );
    }

    #[test]
    fn effective_entitlements_report_local_owner_status() {
        let entitlements = effective_entitlements(None, &[]);
        let status = capability_status(&[
            "research.backtest.run".to_string(),
            "billing.checkout".to_string(),
        ]);

        assert_eq!(entitlements["plan"], "local_full");
        assert_eq!(entitlements["limits"]["evaluation_frequency_tier"], "high");
        assert_eq!(entitlements["workspace"]["strategy.create"], true);
        assert_eq!(
            entitlements["workspace"]["commercialCapabilitiesDenied"],
            true
        );
        assert_eq!(status["research.backtest.run"], true);
        assert_eq!(status["billing.checkout"], false);
    }
}
