// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::adapters;
use crate::ports::{PortDescriptor, PortKind, ServiceRuntime, PORT_CONTRACT_VERSION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::Arc;

pub const RUNTIME_CONFIG_VERSION: &str = "tradeassembly.runtime-config.v1";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProfile {
    #[default]
    Local,
    SelfHosted,
    Serverless,
}

impl RuntimeProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::SelfHosted => "self_hosted",
            Self::Serverless => "serverless",
        }
    }

    fn parse(value: &str) -> Result<(Self, Option<&'static str>), String> {
        match value.trim() {
            "local" => Ok((Self::Local, None)),
            "self_hosted" | "self-hosted" => Ok((Self::SelfHosted, None)),
            "distributed_dev" | "distributed-dev" => {
                Ok((Self::SelfHosted, Some("distributed_dev")))
            }
            "serverless" => Ok((Self::Serverless, None)),
            other => Err(format!("unsupported runtime profile: {other}")),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeConfigLayer {
    pub profile: Option<String>,
    pub database_path: Option<String>,
    pub artifact_root: Option<String>,
    pub oidc_profile: Option<String>,
    pub hub_base_url: Option<String>,
    pub oidc_issuer: Option<String>,
    pub oidc_audience: Option<String>,
    pub oidc_client_id: Option<String>,
    pub oidc_client_secret_ref: Option<String>,
    pub oidc_redirect_uri: Option<String>,
    pub oidc_scopes: Option<String>,
    pub oidc_session_path: Option<String>,
    pub oidc_session_key_path: Option<String>,
    pub oidc_callback_timeout_seconds: Option<u64>,
    pub studio_core_token_ref: Option<String>,
    pub studio_base_url: Option<String>,
    pub legal_receipt_root: Option<String>,
    pub legal_trusted_keys_path: Option<String>,
    pub legal_policy_path: Option<String>,
    pub warden_sidecar_url: Option<String>,
    pub warden_token_ref: Option<String>,
    pub warden_required_version: Option<String>,
    pub postgres_url_ref: Option<String>,
    pub nats_url_ref: Option<String>,
    pub object_store_endpoint: Option<String>,
    pub object_store_credential_ref: Option<String>,
    pub plugin_sandbox_command: Option<String>,
    pub plugin_sandbox_allow_local_egress: Option<bool>,
    pub telemetry_enabled: Option<bool>,
}

impl RuntimeConfigLayer {
    pub fn from_json_file(path: &str) -> Result<Self, String> {
        let body = fs::read_to_string(path)
            .map_err(|error| format!("read runtime config file failed: {error}"))?;
        serde_json::from_str(&body)
            .map_err(|error| format!("parse runtime config file failed: {error}"))
    }

    pub fn from_env() -> Result<Self, String> {
        fn value(name: &str) -> Option<String> {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        }
        fn boolean(name: &str) -> Result<Option<bool>, String> {
            value(name)
                .map(|raw| match raw.as_str() {
                    "1" | "true" | "TRUE" => Ok(true),
                    "0" | "false" | "FALSE" => Ok(false),
                    _ => Err(format!("{name} must be true or false")),
                })
                .transpose()
        }
        Ok(Self {
            profile: value("TRADEASSEMBLY_RUNTIME_PROFILE"),
            database_path: value("TRADEASSEMBLY_DATABASE_PATH"),
            artifact_root: value("TRADEASSEMBLY_ARTIFACT_ROOT"),
            oidc_profile: value("TRADEASSEMBLY_AUTH_PROFILE"),
            hub_base_url: value("TRADEASSEMBLY_HUB_BASE_URL"),
            oidc_issuer: value("TRADEASSEMBLY_AUTH_ISSUER"),
            oidc_audience: value("TRADEASSEMBLY_AUTH_AUDIENCE"),
            oidc_client_id: value("TRADEASSEMBLY_AUTH_CLIENT_ID"),
            oidc_client_secret_ref: value("TRADEASSEMBLY_AUTH_CLIENT_SECRET_REF"),
            oidc_redirect_uri: value("TRADEASSEMBLY_AUTH_CLI_REDIRECT_URI"),
            oidc_scopes: value("TRADEASSEMBLY_AUTH_SCOPES"),
            oidc_session_path: value("TRADEASSEMBLY_AUTH_CLI_SESSION_PATH"),
            oidc_session_key_path: value("TRADEASSEMBLY_AUTH_CLI_SESSION_KEY_PATH"),
            oidc_callback_timeout_seconds: value("TRADEASSEMBLY_AUTH_CLI_CALLBACK_TIMEOUT_SECONDS")
                .map(|raw| {
                    raw.parse::<u64>().map_err(|_| {
                        "TRADEASSEMBLY_AUTH_CLI_CALLBACK_TIMEOUT_SECONDS must be an integer"
                            .to_string()
                    })
                })
                .transpose()?,
            studio_core_token_ref: value("TRADEASSEMBLY_STUDIO_CORE_TOKEN_REF"),
            studio_base_url: value("TRADEASSEMBLY_STUDIO_BASE_URL"),
            legal_receipt_root: value("TRADEASSEMBLY_LEGAL_RECEIPT_ROOT"),
            legal_trusted_keys_path: value("TRADEASSEMBLY_LEGAL_TRUSTED_KEYS_PATH"),
            legal_policy_path: value("TRADEASSEMBLY_LEGAL_POLICY_PATH"),
            warden_sidecar_url: value("TRADEASSEMBLY_WARDEN_SIDECAR_URL"),
            warden_token_ref: value("TRADEASSEMBLY_WARDEN_TOKEN_REF"),
            warden_required_version: value("TRADEASSEMBLY_WARDEN_REQUIRED_VERSION"),
            postgres_url_ref: value("TRADEASSEMBLY_POSTGRES_URL_REF"),
            nats_url_ref: value("TRADEASSEMBLY_NATS_URL_REF"),
            object_store_endpoint: value("TRADEASSEMBLY_OBJECT_STORE_ENDPOINT"),
            object_store_credential_ref: value("TRADEASSEMBLY_OBJECT_STORE_CREDENTIAL_REF"),
            plugin_sandbox_command: value("TRADEASSEMBLY_PLUGIN_SANDBOX_COMMAND"),
            plugin_sandbox_allow_local_egress: boolean(
                "TRADEASSEMBLY_PLUGIN_SANDBOX_ALLOW_LOCAL_EGRESS",
            )?,
            telemetry_enabled: boolean("TRADEASSEMBLY_TELEMETRY_ENABLED")?,
        })
    }

    fn overlay(&mut self, later: Self) {
        macro_rules! replace {
            ($field:ident) => {
                if later.$field.is_some() {
                    self.$field = later.$field;
                }
            };
        }
        replace!(profile);
        replace!(database_path);
        replace!(artifact_root);
        replace!(oidc_profile);
        replace!(hub_base_url);
        replace!(oidc_issuer);
        replace!(oidc_audience);
        replace!(oidc_client_id);
        replace!(oidc_client_secret_ref);
        replace!(oidc_redirect_uri);
        replace!(oidc_scopes);
        replace!(oidc_session_path);
        replace!(oidc_session_key_path);
        replace!(oidc_callback_timeout_seconds);
        replace!(studio_core_token_ref);
        replace!(studio_base_url);
        replace!(legal_receipt_root);
        replace!(legal_trusted_keys_path);
        replace!(legal_policy_path);
        replace!(warden_sidecar_url);
        replace!(warden_token_ref);
        replace!(warden_required_version);
        replace!(postgres_url_ref);
        replace!(nats_url_ref);
        replace!(object_store_endpoint);
        replace!(object_store_credential_ref);
        replace!(plugin_sandbox_command);
        replace!(plugin_sandbox_allow_local_egress);
        replace!(telemetry_enabled);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub profile: RuntimeProfile,
    pub preset: Option<String>,
    pub database_path: Option<String>,
    pub artifact_root: Option<String>,
    pub oidc_profile: String,
    pub hub_base_url: String,
    pub oidc_issuer: String,
    pub oidc_audience: String,
    pub oidc_client_id: String,
    pub oidc_client_secret_ref: Option<String>,
    pub oidc_redirect_uri: String,
    pub oidc_scopes: Vec<String>,
    pub oidc_session_path: String,
    pub oidc_session_key_path: String,
    pub oidc_callback_timeout_seconds: u64,
    pub studio_core_token_ref: Option<String>,
    pub studio_base_url: Option<String>,
    pub legal_receipt_root: String,
    pub legal_trusted_keys_path: String,
    pub legal_policy_path: String,
    pub warden_sidecar_url: String,
    pub warden_token_ref: String,
    pub warden_required_version: String,
    pub postgres_url_ref: Option<String>,
    pub nats_url_ref: Option<String>,
    pub object_store_endpoint: Option<String>,
    pub object_store_credential_ref: Option<String>,
    pub plugin_sandbox_command: String,
    pub plugin_sandbox_allow_local_egress: bool,
    pub telemetry_enabled: bool,
}

impl RuntimeConfig {
    /// Internal assertion binding: AuthKit's verified client_id, not a JWT aud claim.
    pub fn identity_client_binding(&self) -> &str {
        if self.oidc_profile == "workos" {
            &self.oidc_client_id
        } else {
            &self.oidc_audience
        }
    }

    pub fn local(database_path: impl Into<String>) -> Self {
        Self {
            profile: RuntimeProfile::Local,
            preset: None,
            database_path: Some(database_path.into()),
            artifact_root: Some(".tradeassembly/exports".to_string()),
            oidc_profile: "workos".to_string(),
            hub_base_url: "https://hub.tradeassembly.ai".to_string(),
            oidc_issuer: String::new(),
            oidc_audience: String::new(),
            oidc_client_id: String::new(),
            oidc_client_secret_ref: None,
            oidc_redirect_uri: "http://127.0.0.1:8976/callback".to_string(),
            oidc_scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
                "offline_access".to_string(),
            ],
            oidc_session_path: ".tradeassembly/auth/cli-session.bin".to_string(),
            oidc_session_key_path: ".tradeassembly/auth/cli-session.key".to_string(),
            oidc_callback_timeout_seconds: 600,
            studio_core_token_ref: None,
            studio_base_url: None,
            legal_receipt_root: ".tradeassembly/legal/receipts".to_string(),
            legal_trusted_keys_path: ".tradeassembly/legal/trusted-keys.json".to_string(),
            legal_policy_path: ".tradeassembly/legal/policy.json".to_string(),
            warden_sidecar_url: "http://127.0.0.1:8181".to_string(),
            warden_token_ref: "file://.tradeassembly/warden.token".to_string(),
            warden_required_version: crate::finance_authority::REQUIRED_WARDEN_SERVICE_VERSION
                .to_string(),
            postgres_url_ref: None,
            nats_url_ref: None,
            object_store_endpoint: None,
            object_store_credential_ref: None,
            plugin_sandbox_command: "srt".to_string(),
            plugin_sandbox_allow_local_egress: false,
            telemetry_enabled: false,
        }
    }

    pub fn local_runner(
        database_path: impl Into<String>,
        environment: RuntimeConfigLayer,
    ) -> Result<Self, String> {
        Self::resolve(
            None,
            environment,
            RuntimeConfigLayer {
                profile: Some("local".to_string()),
                database_path: Some(database_path.into()),
                ..RuntimeConfigLayer::default()
            },
        )
    }

    pub fn resolve(
        file: Option<RuntimeConfigLayer>,
        environment: RuntimeConfigLayer,
        cli: RuntimeConfigLayer,
    ) -> Result<Self, String> {
        let mut merged = file.unwrap_or_default();
        merged.overlay(environment);
        merged.overlay(cli);
        let (profile, preset) =
            RuntimeProfile::parse(merged.profile.as_deref().unwrap_or("local"))?;
        let mut defaults = match profile {
            RuntimeProfile::Local => Self::local(
                merged
                    .database_path
                    .clone()
                    .unwrap_or_else(|| ".tradeassembly/tradeassembly.db".to_string()),
            ),
            RuntimeProfile::SelfHosted | RuntimeProfile::Serverless => Self {
                profile,
                preset: preset.map(str::to_string),
                database_path: None,
                artifact_root: None,
                oidc_profile: "workos".to_string(),
                hub_base_url: "https://hub.tradeassembly.ai".to_string(),
                oidc_issuer: String::new(),
                oidc_audience: String::new(),
                oidc_client_id: String::new(),
                oidc_client_secret_ref: None,
                oidc_redirect_uri: String::new(),
                oidc_scopes: vec![
                    "openid".to_string(),
                    "profile".to_string(),
                    "email".to_string(),
                    "offline_access".to_string(),
                ],
                oidc_session_path: ".tradeassembly/auth/cli-session.bin".to_string(),
                oidc_session_key_path: ".tradeassembly/auth/cli-session.key".to_string(),
                oidc_callback_timeout_seconds: 600,
                studio_core_token_ref: None,
                studio_base_url: None,
                legal_receipt_root: ".tradeassembly/legal/receipts".to_string(),
                legal_trusted_keys_path: ".tradeassembly/legal/trusted-keys.json".to_string(),
                legal_policy_path: ".tradeassembly/legal/policy.json".to_string(),
                warden_sidecar_url: "http://127.0.0.1:8181".to_string(),
                warden_token_ref: "file://.tradeassembly/warden.token".to_string(),
                warden_required_version: crate::finance_authority::REQUIRED_WARDEN_SERVICE_VERSION
                    .to_string(),
                postgres_url_ref: None,
                nats_url_ref: None,
                object_store_endpoint: None,
                object_store_credential_ref: None,
                plugin_sandbox_command: "srt".to_string(),
                plugin_sandbox_allow_local_egress: false,
                telemetry_enabled: false,
            },
        };
        // Development identity is opt-in, independent of where the runtime runs.
        if merged.oidc_profile.as_deref() == Some("keycloak_local") {
            defaults.oidc_issuer = "http://127.0.0.1:8080/realms/tradeassembly-dev".to_string();
            defaults.oidc_client_id = "tradeassembly-local".to_string();
            defaults.oidc_audience = "tradeassembly-local".to_string();
        }
        if merged.oidc_profile.as_deref() == Some("hub") {
            merged.oidc_profile = Some("workos".to_string());
        }
        let config = Self {
            profile,
            preset: preset.map(str::to_string),
            database_path: merged.database_path.or(defaults.database_path),
            artifact_root: merged.artifact_root.or(defaults.artifact_root),
            oidc_profile: merged.oidc_profile.unwrap_or(defaults.oidc_profile),
            hub_base_url: merged.hub_base_url.unwrap_or(defaults.hub_base_url),
            oidc_issuer: merged.oidc_issuer.unwrap_or(defaults.oidc_issuer),
            oidc_audience: merged.oidc_audience.unwrap_or(defaults.oidc_audience),
            oidc_client_id: merged.oidc_client_id.unwrap_or(defaults.oidc_client_id),
            oidc_client_secret_ref: merged.oidc_client_secret_ref,
            oidc_redirect_uri: merged
                .oidc_redirect_uri
                .unwrap_or(defaults.oidc_redirect_uri),
            oidc_scopes: merged
                .oidc_scopes
                .map(|value| value.split_whitespace().map(str::to_string).collect())
                .unwrap_or(defaults.oidc_scopes),
            oidc_session_path: merged
                .oidc_session_path
                .unwrap_or(defaults.oidc_session_path),
            oidc_session_key_path: merged
                .oidc_session_key_path
                .unwrap_or(defaults.oidc_session_key_path),
            oidc_callback_timeout_seconds: merged
                .oidc_callback_timeout_seconds
                .unwrap_or(defaults.oidc_callback_timeout_seconds),
            studio_core_token_ref: merged.studio_core_token_ref,
            studio_base_url: merged.studio_base_url,
            legal_receipt_root: merged
                .legal_receipt_root
                .unwrap_or(defaults.legal_receipt_root),
            legal_trusted_keys_path: merged
                .legal_trusted_keys_path
                .unwrap_or(defaults.legal_trusted_keys_path),
            legal_policy_path: merged
                .legal_policy_path
                .unwrap_or(defaults.legal_policy_path),
            warden_sidecar_url: merged
                .warden_sidecar_url
                .unwrap_or(defaults.warden_sidecar_url),
            warden_token_ref: merged.warden_token_ref.unwrap_or(defaults.warden_token_ref),
            warden_required_version: merged
                .warden_required_version
                .unwrap_or(defaults.warden_required_version),
            postgres_url_ref: merged.postgres_url_ref,
            nats_url_ref: merged.nats_url_ref,
            object_store_endpoint: merged.object_store_endpoint,
            object_store_credential_ref: merged.object_store_credential_ref,
            plugin_sandbox_command: merged
                .plugin_sandbox_command
                .unwrap_or(defaults.plugin_sandbox_command),
            plugin_sandbox_allow_local_egress: merged
                .plugin_sandbox_allow_local_egress
                .unwrap_or(defaults.plugin_sandbox_allow_local_egress),
            telemetry_enabled: merged.telemetry_enabled.unwrap_or(false),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.plugin_sandbox_command.trim().is_empty() {
            return Err("plugin sandbox command is required".to_string());
        }
        if self.plugin_sandbox_allow_local_egress && self.profile != RuntimeProfile::Local {
            return Err(
                "plugin sandbox local egress override is restricted to the local profile"
                    .to_string(),
            );
        }
        if !matches!(
            self.oidc_profile.as_str(),
            "local_owner"
                | "workos"
                | "tradeassembly_hub_production"
                | "tradeassembly_hub_staging"
                | "keycloak_local"
                | "oidc_test"
                | "configured_oidc"
        ) {
            return Err("unsupported OIDC profile".to_string());
        }
        let unconfigured_workos = self.oidc_profile == "workos"
            && self.oidc_issuer.is_empty()
            && self.oidc_client_id.is_empty();
        let local_owner = self.oidc_profile == "local_owner";
        if local_owner
            && (self.profile != RuntimeProfile::Local
                || self
                    .database_path
                    .as_deref()
                    .is_none_or(|path| path.is_empty() || path == ":memory:")
                || !self.oidc_issuer.is_empty()
                || !self.oidc_client_id.is_empty()
                || self.oidc_client_secret_ref.is_some())
        {
            return Err("local_owner_requires_local_database_without_hosted_identity".to_string());
        }
        if self.oidc_profile == "workos" && self.oidc_client_secret_ref.is_some() {
            return Err("installed WorkOS clients must not use a client secret".to_string());
        }
        if !unconfigured_workos && !local_owner {
            if !self.oidc_issuer.starts_with("http://") && !self.oidc_issuer.starts_with("https://")
            {
                return Err("OIDC issuer must be an absolute HTTP(S) URL".to_string());
            }
            let issuer = url::Url::parse(&self.oidc_issuer)
                .map_err(|_| "OIDC issuer must be an absolute HTTP(S) URL".to_string())?;
            let loopback_issuer = issuer
                .host_str()
                .map(|host| host.trim_matches(['[', ']']))
                .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
            if issuer.scheme() == "http"
                && !(loopback_issuer
                    && matches!(self.oidc_profile.as_str(), "keycloak_local" | "oidc_test"))
            {
                return Err(
                    "OIDC issuer must use HTTPS except for named loopback profiles".to_string(),
                );
            }
            if (self.oidc_profile != "workos" && self.oidc_audience.trim().is_empty())
                || self.oidc_client_id.trim().is_empty()
            {
                return Err("OIDC audience and client id are required".to_string());
            }
        }
        crate::hub_identity::validate_base_url(&self.hub_base_url)?;
        let redirect = url::Url::parse(&self.oidc_redirect_uri)
            .map_err(|_| "CLI OIDC redirect URI must be absolute".to_string())?;
        if redirect.scheme() != "http"
            || (self.oidc_profile == "workos" && redirect.host_str() != Some("127.0.0.1"))
            || !redirect
                .host_str()
                .map(|host| host.trim_matches(['[', ']']))
                .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"))
            || redirect.port().is_none()
            || redirect.query().is_some()
            || redirect.fragment().is_some()
        {
            return Err(
                "CLI OIDC redirect URI must be loopback HTTP with an explicit port".to_string(),
            );
        }
        if !self.oidc_scopes.iter().any(|scope| scope == "openid") {
            return Err("OIDC scopes must include openid".to_string());
        }
        if self.oidc_session_path.trim().is_empty() || self.oidc_session_key_path.trim().is_empty()
        {
            return Err("CLI OIDC session paths are required".to_string());
        }
        if self.oidc_session_path == self.oidc_session_key_path {
            return Err("CLI OIDC session and key paths must differ".to_string());
        }
        if self.legal_receipt_root.trim().is_empty()
            || self.legal_trusted_keys_path.trim().is_empty()
            || self.legal_policy_path.trim().is_empty()
        {
            return Err("legal receipt paths must be configured".to_string());
        }
        if !(1..=600).contains(&self.oidc_callback_timeout_seconds) {
            return Err("CLI OIDC callback timeout must be between 1 and 600 seconds".to_string());
        }
        if !self.warden_sidecar_url.starts_with("http://")
            && !self.warden_sidecar_url.starts_with("https://")
        {
            return Err("Warden sidecar URL must be an absolute HTTP(S) URL".to_string());
        }
        if self.warden_required_version.trim().is_empty() {
            return Err("required Warden service version is empty".to_string());
        }
        validate_secret_ref(&self.warden_token_ref)?;
        for reference in [
            self.oidc_client_secret_ref.as_deref(),
            self.studio_core_token_ref.as_deref(),
            self.postgres_url_ref.as_deref(),
            self.nats_url_ref.as_deref(),
            self.object_store_credential_ref.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            validate_secret_ref(reference)?;
        }
        match self.profile {
            RuntimeProfile::Local => {
                require(&self.database_path, "local database path")?;
                require(&self.artifact_root, "local artifact root")?;
                if self.postgres_url_ref.is_some() || self.nats_url_ref.is_some() {
                    return Err("local profile cannot select distributed adapters".to_string());
                }
            }
            RuntimeProfile::SelfHosted => {
                require(&self.postgres_url_ref, "Postgres URL secret reference")?;
                require(&self.nats_url_ref, "NATS JetStream URL secret reference")?;
                require(&self.object_store_endpoint, "object store endpoint")?;
                if !self
                    .object_store_endpoint
                    .as_deref()
                    .is_some_and(|endpoint| endpoint.starts_with("file://"))
                {
                    require(
                        &self.object_store_credential_ref,
                        "object store credential reference",
                    )?;
                }
            }
            RuntimeProfile::Serverless => {
                if self.database_path.is_some()
                    || self.postgres_url_ref.is_some()
                    || self.nats_url_ref.is_some()
                {
                    return Err(
                        "serverless profile requires externally injected managed adapters"
                            .to_string(),
                    );
                }
            }
        }
        Ok(())
    }

    pub fn redacted_configuration(&self) -> Value {
        json!({
            "configVersion": RUNTIME_CONFIG_VERSION,
            "profile": self.profile.as_str(),
            "preset": self.preset,
            "databasePathConfigured": self.database_path.is_some(),
            "artifactRootConfigured": self.artifact_root.is_some(),
            "oidcProfile": self.oidc_profile,
            "hubBaseUrl": self.hub_base_url,
            "oidcIssuer": self.oidc_issuer,
            "oidcAudience": self.oidc_audience,
            "oidcClientId": self.oidc_client_id,
            "oidcClientSecretRefConfigured": self.oidc_client_secret_ref.is_some(),
            "oidcCliRedirectUri": self.oidc_redirect_uri,
            "oidcScopes": self.oidc_scopes,
            "oidcCliSessionPath": self.oidc_session_path,
            "oidcCliCallbackTimeoutSeconds": self.oidc_callback_timeout_seconds,
            "studioCoreTokenRefConfigured": self.studio_core_token_ref.is_some(),
            "legalReceiptRoot": self.legal_receipt_root,
            "legalTrustedKeysPath": self.legal_trusted_keys_path,
            "legalPolicyPath": self.legal_policy_path,
            "wardenSidecarUrl": self.warden_sidecar_url,
            "wardenTokenRefConfigured": true,
            "wardenRequiredVersion": self.warden_required_version,
            "postgresUrlRefConfigured": self.postgres_url_ref.is_some(),
            "natsUrlRefConfigured": self.nats_url_ref.is_some(),
            "objectStoreEndpointConfigured": self.object_store_endpoint.is_some(),
            "objectStoreCredentialRefConfigured": self.object_store_credential_ref.is_some(),
            "telemetryEnabled": self.telemetry_enabled,
        })
    }
}

fn require<T>(value: &Option<T>, name: &str) -> Result<(), String> {
    value
        .as_ref()
        .map(|_| ())
        .ok_or_else(|| format!("{name} is required"))
}

fn validate_secret_ref(reference: &str) -> Result<(), String> {
    let allowed = ["env://", "keychain://", "file://", "vault://", "secret://"];
    if allowed.iter().any(|prefix| reference.starts_with(prefix))
        && reference
            .split_once("://")
            .is_some_and(|(_, key)| !key.trim().is_empty())
    {
        Ok(())
    } else {
        Err("secret values must be opaque references using an approved scheme".to_string())
    }
}

pub trait SecretResolver: Send + Sync {
    fn resolve(&self, reference: &str) -> Result<String, String>;
}

#[derive(Clone, Debug, Default)]
pub struct EnvSecretResolver;

impl SecretResolver for EnvSecretResolver {
    fn resolve(&self, reference: &str) -> Result<String, String> {
        let (scheme, key) = reference
            .split_once("://")
            .ok_or_else(|| "secret reference is malformed".to_string())?;
        match scheme {
            "env" => {
                std::env::var(key).map_err(|_| format!("environment secret is unavailable: {key}"))
            }
            "file" => fs::read_to_string(key)
                .map(|value| value.trim_end_matches(['\r', '\n']).to_string())
                .map_err(|_| "file-backed secret is unavailable".to_string()),
            _ => Err(format!(
                "secret reference scheme is unsupported by bootstrap: {scheme}"
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapSecrets {
    pub postgres_url: String,
    pub nats_url: String,
    pub oidc_client_secret: Option<String>,
    pub studio_core_token: Option<String>,
    pub warden_token: String,
    pub object_store_credential: Option<String>,
}

impl BootstrapSecrets {
    fn resolve(config: &RuntimeConfig, resolver: &dyn SecretResolver) -> Result<Self, String> {
        Ok(Self {
            postgres_url: resolver.resolve(
                config
                    .postgres_url_ref
                    .as_deref()
                    .ok_or_else(|| "Postgres URL secret reference is required".to_string())?,
            )?,
            nats_url: resolver.resolve(
                config
                    .nats_url_ref
                    .as_deref()
                    .ok_or_else(|| "NATS JetStream URL secret reference is required".to_string())?,
            )?,
            oidc_client_secret: config
                .oidc_client_secret_ref
                .as_deref()
                .map(|reference| resolver.resolve(reference))
                .transpose()?,
            studio_core_token: config
                .studio_core_token_ref
                .as_deref()
                .map(|reference| resolver.resolve(reference))
                .transpose()?,
            warden_token: resolver.resolve(&config.warden_token_ref)?,
            object_store_credential: config
                .object_store_credential_ref
                .as_deref()
                .map(|reference| resolver.resolve(reference))
                .transpose()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRuntimeManifest {
    pub config_version: String,
    pub port_contract_version: String,
    pub profile: String,
    pub preset: Option<String>,
    pub configuration_digest: String,
    pub adapters: Vec<PortDescriptor>,
    pub health: String,
    pub conformance: String,
}

pub struct RuntimeBuilder {
    config: RuntimeConfig,
    injected: Option<ServiceRuntime>,
    secret_resolver: Option<Arc<dyn SecretResolver>>,
    finance_authority: Option<Arc<dyn crate::finance_authority::FinanceAuthorityPort>>,
}

impl RuntimeBuilder {
    pub fn new(config: RuntimeConfig) -> Self {
        Self {
            config,
            injected: None,
            secret_resolver: None,
            finance_authority: None,
        }
    }

    pub fn with_runtime(mut self, runtime: ServiceRuntime) -> Self {
        self.injected = Some(runtime);
        self
    }

    pub fn with_secret_resolver(mut self, resolver: Arc<dyn SecretResolver>) -> Self {
        self.secret_resolver = Some(resolver);
        self
    }

    pub fn with_finance_authority(
        mut self,
        authority: Arc<dyn crate::finance_authority::FinanceAuthorityPort>,
    ) -> Self {
        self.finance_authority = Some(authority);
        self
    }

    pub fn build(self) -> Result<(ServiceRuntime, ResolvedRuntimeManifest), String> {
        self.config.validate()?;
        let runtime = match (self.config.profile, self.injected) {
            (RuntimeProfile::Local, None) => adapters::local::runtime_from_config_with_authority(
                &self.config,
                self.finance_authority,
            )?,
            (RuntimeProfile::Local, Some(_)) => {
                return Err("local profile uses the registered local adapter bundle".to_string())
            }
            (_, Some(runtime)) => runtime,
            (RuntimeProfile::SelfHosted, None) => {
                let resolver = self
                    .secret_resolver
                    .unwrap_or_else(|| Arc::new(EnvSecretResolver));
                let secrets = BootstrapSecrets::resolve(&self.config, resolver.as_ref())?;
                adapters::self_hosted::runtime_from_config_with_authority(
                    &self.config,
                    &secrets,
                    self.finance_authority,
                )?
            }
            (RuntimeProfile::Serverless, None) => {
                return Err(
                    "serverless profile has no injected conformant adapter bundle".to_string(),
                )
            }
        };
        let descriptors = descriptors(&runtime);
        validate_registry(self.config.profile, &descriptors)?;
        let redacted = serde_json::to_vec(&self.config.redacted_configuration())
            .map_err(|error| error.to_string())?;
        let digest = format!("sha256:{:x}", Sha256::digest(redacted));
        Ok((
            runtime,
            ResolvedRuntimeManifest {
                config_version: RUNTIME_CONFIG_VERSION.to_string(),
                port_contract_version: PORT_CONTRACT_VERSION.to_string(),
                profile: self.config.profile.as_str().to_string(),
                preset: self.config.preset,
                configuration_digest: digest,
                adapters: descriptors,
                health: "not_checked".to_string(),
                conformance: "not_run".to_string(),
            },
        ))
    }
}

fn descriptors(runtime: &ServiceRuntime) -> Vec<PortDescriptor> {
    let mut all = Vec::new();
    all.extend(runtime.storage.descriptors());
    all.extend(runtime.object_authorization.descriptors());
    all.extend(runtime.capability_resolver.descriptors());
    all.extend(runtime.credentials.descriptors());
    all.extend(runtime.historical_data.descriptors());
    all.extend(runtime.dataset_snapshots.descriptors());
    all.extend(runtime.backtests.descriptors());
    all.extend(runtime.providers.descriptors());
    all.extend(runtime.plugin_operations.descriptors());
    all.extend(runtime.journal.descriptors());
    all.extend(runtime.bus.descriptors());
    all.extend(runtime.queue.descriptors());
    all.extend(runtime.events.descriptors());
    all.extend(runtime.scheduler.descriptors());
    all.extend(runtime.outbox.descriptors());
    all.extend(runtime.inbox.descriptors());
    all.extend(runtime.leases.descriptors());
    all.extend(runtime.identity.descriptors());
    all.extend(runtime.policy.descriptors());
    all.extend(runtime.evidence.descriptors());
    all.extend(runtime.plugins.descriptors());
    all.extend(runtime.plugin_packages.descriptors());
    all.extend(runtime.telemetry.descriptors());
    all.extend(runtime.clock.descriptors());
    all.extend(runtime.exports.descriptors());
    all.sort_by(|left, right| {
        (left.kind, left.adapter_id.as_str()).cmp(&(right.kind, right.adapter_id.as_str()))
    });
    all
}

fn validate_registry(
    profile: RuntimeProfile,
    descriptors: &[PortDescriptor],
) -> Result<(), String> {
    let required = BTreeSet::from([
        PortKind::Storage,
        PortKind::DurableQueue,
        PortKind::EventStream,
        PortKind::Scheduler,
        PortKind::Outbox,
        PortKind::Inbox,
        PortKind::Lease,
        PortKind::Identity,
        PortKind::Policy,
        PortKind::Evidence,
        PortKind::Credentials,
        PortKind::Plugins,
        PortKind::Telemetry,
        PortKind::Clock,
        PortKind::Exports,
    ]);
    let mut by_kind = BTreeMap::<PortKind, usize>::new();
    for descriptor in descriptors {
        if descriptor.contract_version != PORT_CONTRACT_VERSION {
            return Err(format!(
                "adapter {} uses an unsupported port contract",
                descriptor.adapter_id
            ));
        }
        if !descriptor
            .supported_profiles
            .iter()
            .any(|candidate| candidate == profile.as_str())
        {
            return Err(format!(
                "adapter {} does not support profile {}",
                descriptor.adapter_id,
                profile.as_str()
            ));
        }
        *by_kind.entry(descriptor.kind).or_default() += 1;
    }
    let missing = required
        .into_iter()
        .filter(|kind| !by_kind.contains_key(kind))
        .map(|kind| format!("{kind:?}"))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "runtime adapter registry is incomplete: {}",
            missing.join(", ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_customer_defaults_do_not_select_development_identity() {
        let config = RuntimeConfig::local(":memory:");
        assert_eq!(config.oidc_profile, "workos");
        assert!(config.oidc_issuer.is_empty());
        assert!(config.oidc_client_id.is_empty());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn explicit_keycloak_profile_has_development_defaults() {
        let config = RuntimeConfig::resolve(
            None,
            RuntimeConfigLayer::default(),
            RuntimeConfigLayer {
                oidc_profile: Some("keycloak_local".to_string()),
                ..RuntimeConfigLayer::default()
            },
        )
        .unwrap();
        assert_eq!(
            config.oidc_issuer,
            "http://127.0.0.1:8080/realms/tradeassembly-dev"
        );
        assert_eq!(config.oidc_client_id, "tradeassembly-local");
    }

    #[test]
    fn workos_uses_public_client_binding_not_shared_audience() {
        let mut config = RuntimeConfig::local(":memory:");
        config.oidc_issuer = "https://identity.example/user_management/client_test".to_string();
        config.oidc_client_id = "client_test".to_string();
        assert!(config.validate().is_ok());
        config.oidc_client_secret_ref = Some("env://SECRET".to_string());
        assert!(config.validate().is_err());
        config.oidc_client_secret_ref = None;
        config.oidc_redirect_uri = "http://localhost:8976/callback".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn precedence_is_file_then_environment_then_cli() {
        let file = RuntimeConfigLayer {
            database_path: Some("file.db".to_string()),
            telemetry_enabled: Some(true),
            ..RuntimeConfigLayer::default()
        };
        let environment = RuntimeConfigLayer {
            database_path: Some("env.db".to_string()),
            ..RuntimeConfigLayer::default()
        };
        let cli = RuntimeConfigLayer {
            database_path: Some("cli.db".to_string()),
            telemetry_enabled: Some(false),
            ..RuntimeConfigLayer::default()
        };
        let config = RuntimeConfig::resolve(Some(file), environment, cli).expect("config");
        assert_eq!(config.database_path.as_deref(), Some("cli.db"));
        assert!(!config.telemetry_enabled);
    }

    #[test]
    fn local_runner_preserves_adapter_environment_but_owns_profile_and_database() {
        let config = RuntimeConfig::local_runner(
            "runner.db",
            RuntimeConfigLayer {
                profile: Some("self_hosted".to_string()),
                database_path: Some("environment.db".to_string()),
                warden_sidecar_url: Some("http://127.0.0.1:9091".to_string()),
                warden_token_ref: Some("env://TRADEASSEMBLY_RUNNER_WARDEN_TOKEN".to_string()),
                telemetry_enabled: Some(true),
                ..RuntimeConfigLayer::default()
            },
        )
        .expect("runner config");

        assert_eq!(config.profile, RuntimeProfile::Local);
        assert_eq!(config.database_path.as_deref(), Some("runner.db"));
        assert_eq!(config.warden_sidecar_url, "http://127.0.0.1:9091");
        assert_eq!(
            config.warden_token_ref,
            "env://TRADEASSEMBLY_RUNNER_WARDEN_TOKEN"
        );
        assert!(config.telemetry_enabled);
    }

    #[test]
    fn unsupported_and_incomplete_profiles_fail_closed() {
        assert!(RuntimeConfig::resolve(
            None,
            RuntimeConfigLayer::default(),
            RuntimeConfigLayer {
                profile: Some("kafka-production".to_string()),
                ..RuntimeConfigLayer::default()
            },
        )
        .is_err());
        assert!(RuntimeConfig::resolve(
            None,
            RuntimeConfigLayer::default(),
            RuntimeConfigLayer {
                profile: Some("self_hosted".to_string()),
                oidc_profile: Some("configured_oidc".to_string()),
                oidc_issuer: Some("https://identity.example".to_string()),
                oidc_audience: Some("tradeassembly".to_string()),
                oidc_client_id: Some("tradeassembly".to_string()),
                oidc_redirect_uri: Some("http://127.0.0.1:8976/auth/cli/callback".to_string(),),
                ..RuntimeConfigLayer::default()
            },
        )
        .is_err());
    }

    #[test]
    fn secret_values_are_rejected_and_redacted() {
        let mut config = RuntimeConfig::local(":memory:");
        config.oidc_client_secret_ref = Some("raw-secret".to_string());
        assert!(config.validate().is_err());

        config.oidc_client_secret_ref = Some("keychain://tradeassembly/oidc".to_string());
        let redacted = config.redacted_configuration().to_string();
        assert!(!redacted.contains("tradeassembly/oidc"));
        assert!(redacted.contains("oidcClientSecretRefConfigured"));
    }

    #[test]
    fn self_hosted_config_accepts_file_artifact_equivalent() {
        let config = RuntimeConfig::resolve(
            None,
            RuntimeConfigLayer::default(),
            RuntimeConfigLayer {
                profile: Some("self_hosted".to_string()),
                oidc_profile: Some("configured_oidc".to_string()),
                oidc_issuer: Some("https://identity.example".to_string()),
                oidc_audience: Some("tradeassembly".to_string()),
                oidc_client_id: Some("tradeassembly".to_string()),
                oidc_redirect_uri: Some("http://127.0.0.1:8976/auth/cli/callback".to_string()),
                postgres_url_ref: Some("env://TRADEASSEMBLY_TEST_PG".to_string()),
                nats_url_ref: Some("env://TRADEASSEMBLY_TEST_NATS".to_string()),
                object_store_endpoint: Some("file:///tmp/tradeassembly-artifacts".to_string()),
                ..RuntimeConfigLayer::default()
            },
        )
        .expect("self-hosted config");
        assert_eq!(config.profile.as_str(), "self_hosted");
    }

    #[test]
    fn local_builder_emits_complete_redacted_manifest() {
        let config = RuntimeConfig::local(":memory:");
        let (_, manifest) = RuntimeBuilder::new(config)
            .with_finance_authority(Arc::new(crate::finance_authority::TestFinanceAuthority))
            .build()
            .expect("runtime");
        assert_eq!(manifest.profile, "local");
        assert_eq!(manifest.health, "not_checked");
        assert_eq!(manifest.conformance, "not_run");
        let kinds = manifest
            .adapters
            .iter()
            .map(|adapter| adapter.kind)
            .collect::<BTreeSet<_>>();
        assert!(kinds.contains(&PortKind::DurableQueue));
        assert!(kinds.contains(&PortKind::Identity));
        assert!(kinds.contains(&PortKind::Exports));
    }
}
