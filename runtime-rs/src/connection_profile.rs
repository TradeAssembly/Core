//! Public deployment configuration supplied by a distribution, never secrets.
use crate::runtime_config::RuntimeConfig;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionProfile {
    pub environment: String,
    pub issuer: String,
    pub client_id: String,
    pub organization_id: String,
    pub hub_base_url: String,
    pub redirect_uri: String,
    pub relay_origins: Vec<String>,
    pub authorization_origins: Vec<String>,
    pub entitlement_checks: Vec<EntitlementCheck>,
    pub checkout_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> ConnectionProfile {
        serde_json::from_value(serde_json::json!({
            "environment":"staging", "issuer":"https://identity.example/issuer",
            "clientId":"native-client", "organizationId":"organization",
            "hubBaseUrl":"https://hub.example", "redirectUri":"http://127.0.0.1:8976/callback",
            "relayOrigins":["https://relay.example"], "authorizationOrigins":["https://broker.example"],
            "entitlementChecks":[], "checkoutUrl":null
        })).unwrap()
    }

    #[test]
    fn accepts_public_native_profile_and_rejects_unsafe_origins() {
        assert!(profile().validate().is_ok());
        for origin in [
            "http://relay.example",
            "https://user:secret@relay.example",
            "https://relay.example/path",
            "https://relay.example?token=secret",
            "https://relay.example#fragment",
        ] {
            let mut candidate = profile();
            candidate.relay_origins = vec![origin.into()];
            assert!(candidate.validate().is_err(), "accepted {origin}");
        }
        let mut candidate = profile();
        candidate.redirect_uri = "https://external.example/callback".into();
        assert!(candidate.validate().is_err());
    }

    #[test]
    fn rejects_oversized_and_secret_bearing_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile.json");
        std::fs::write(&path, vec![b' '; 16385]).unwrap();
        assert!(ConnectionProfile::read(&path).is_err());
        let mut value = serde_json::to_value(profile()).unwrap();
        value["clientSecret"] = serde_json::json!("must-not-be-packaged");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(ConnectionProfile::read(&path).is_err());
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntitlementCheck {
    pub product_id: String,
    pub capability: String,
}

impl ConnectionProfile {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.environment.as_str(), "staging" | "production")
            || self.client_id.is_empty()
            || self.organization_id.is_empty()
        {
            return Err("connection_profile_invalid".into());
        }
        crate::hub_identity::validate_base_url(&self.hub_base_url)?;
        crate::cli_identity::validated_loopback_redirect(&self.redirect_uri)?;
        for value in std::iter::once(&self.issuer)
            .chain(self.relay_origins.iter())
            .chain(self.authorization_origins.iter())
            .chain(self.checkout_url.iter())
        {
            let url = url::Url::parse(value).map_err(|_| "connection_profile_invalid")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
                || url.query().is_some()
            {
                return Err("connection_profile_invalid".into());
            }
        }
        if self.relay_origins.is_empty() || self.authorization_origins.is_empty() {
            return Err("connection_profile_invalid".into());
        }
        for origin in self
            .relay_origins
            .iter()
            .chain(self.authorization_origins.iter())
        {
            let url = url::Url::parse(origin).map_err(|_| "connection_profile_invalid")?;
            if url.path() != "/" {
                return Err("connection_profile_invalid".into());
            }
        }
        Ok(())
    }

    pub fn read(path: &std::path::Path) -> Result<Self, String> {
        use std::io::Read;
        let file = std::fs::File::open(path).map_err(|_| "connection_profile_unavailable")?;
        let mut bytes = Vec::new();
        file.take(16385)
            .read_to_end(&mut bytes)
            .map_err(|_| "connection_profile_unavailable")?;
        if bytes.len() > 16384 {
            return Err("connection_profile_invalid".into());
        }
        let profile: Self =
            serde_json::from_slice(&bytes).map_err(|_| "connection_profile_invalid")?;
        profile.validate()?;
        Ok(profile)
    }

    pub(crate) fn configure(&self, base: &RuntimeConfig) -> RuntimeConfig {
        let mut config = base.clone();
        config.oidc_profile = "workos".into();
        config.oidc_session_store = "bitwarden".into();
        config.oidc_issuer = self.issuer.clone();
        config.oidc_client_id = self.client_id.clone();
        config.oidc_organization_id = Some(self.organization_id.clone());
        config.hub_base_url = self.hub_base_url.clone();
        config.oidc_redirect_uri = self.redirect_uri.clone();
        config.oidc_callback_timeout_seconds = 600;
        if base.oidc_profile == "local_owner" {
            config.oidc_session_path = format!("{}.hosted", base.oidc_session_path);
        }
        config
    }
}
