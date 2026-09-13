// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde_json::{json, Value};

const LEGAL_BOUNDARY: &str =
    "TradeAssembly never tells users what to trade, when to trade, or how much to trade.";

#[derive(Clone, Debug, Default)]
pub struct LocalRuntime {
    strategies: usize,
}

impl LocalRuntime {
    pub fn seed_btc_exit_demo(&mut self) {
        self.strategies = 1;
    }
}

#[derive(Clone, Debug)]
pub struct AdminApi {
    runtime: LocalRuntime,
}

impl AdminApi {
    pub fn new(runtime: LocalRuntime) -> Self {
        Self { runtime }
    }

    pub fn health(&self) -> Value {
        json!({"status": 200, "body": {"status": "ok", "surface": "local-admin"}})
    }

    pub fn status(&self) -> Value {
        json!({
            "status": 200,
            "body": {
                "surface": "local-admin",
                "readiness": {"ok": true},
                "counts": {"strategies": self.runtime.strategies},
                "legalBoundary": LEGAL_BOUNDARY
            }
        })
    }

    pub fn providers(&self) -> Value {
        json!({
            "status": 200,
            "body": {
                "providers": [],
                "credentialStatus": {"provider_ref": "plugin-instance", "custody": "local/customer-managed"}
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct ControlApp {
    admin: AdminApi,
    allow_providers: Vec<String>,
}

impl ControlApp {
    pub fn new(runtime: LocalRuntime, allow_providers: Vec<String>) -> Self {
        Self {
            admin: AdminApi::new(runtime),
            allow_providers,
        }
    }

    pub fn admin(&self) -> &AdminApi {
        &self.admin
    }

    pub fn health(&self) -> Value {
        json!({"status": 200, "body": {"status": "ok", "surface": "local-control"}})
    }

    pub fn oauth_health(&self) -> Value {
        json!({
            "status": 200,
            "body": {
                "status": "ok",
                "mode": "local-self-hosted",
                "managed_app_credentials": false,
                "allowProviders": self.allow_providers
            }
        })
    }

    pub fn oauth_authorize(
        &self,
        provider: &str,
        redirect_uri: &str,
        state: &str,
        stub: bool,
    ) -> Value {
        if !self.allow_providers.iter().any(|item| item == provider) {
            return json!({"status": 400, "body": {"detail": "provider not allowed"}});
        }
        json!({
            "status": 200,
            "body": {
                "provider": provider,
                "redirect_uri": redirect_uri,
                "state": state,
                "stub": stub,
                "mode": "local-self-hosted",
                "managed_app_credentials": false
            }
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn admin_api_exposes_local_runtime_status() {
        let mut runtime = super::LocalRuntime::default();
        runtime.seed_btc_exit_demo();
        let admin = super::AdminApi::new(runtime);

        let health = admin.health();
        let status = admin.status();
        let providers = admin.providers();

        assert_eq!(health["status"], 200);
        assert_eq!(health["body"]["surface"], "local-admin");
        assert_eq!(status["body"]["readiness"]["ok"], true);
        assert_eq!(status["body"]["counts"]["strategies"], 1);
        assert_eq!(
            status["body"]["legalBoundary"],
            "TradeAssembly never tells users what to trade, when to trade, or how much to trade."
        );
        assert_eq!(
            providers["body"]["credentialStatus"]["provider_ref"],
            "plugin-instance"
        );
    }

    #[test]
    fn control_app_mounts_admin_and_self_hosted_oauth() {
        let service = super::LocalRuntime::default();
        let control = super::ControlApp::new(service, vec!["example-provider".to_string()]);

        let root_health = control.health();
        let admin_health = control.admin().health();
        let oauth_health = control.oauth_health();
        let oauth_authorize = control.oauth_authorize(
            "example-provider",
            "http://localhost/callback",
            "owner",
            true,
        );

        assert_eq!(root_health["body"]["surface"], "local-control");
        assert_eq!(admin_health["body"]["surface"], "local-admin");
        assert_eq!(oauth_health["body"]["mode"], "local-self-hosted");
        assert_eq!(oauth_authorize["status"], 200);
        assert_eq!(oauth_authorize["body"]["managed_app_credentials"], false);
    }
}
