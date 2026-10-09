use super::{execution, live_authorization, ServiceResponse, TradeAssemblyService};
use crate::agent_runner::{self, external_session::ExternalAgentSession};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext};
use serde_json::json;

impl TradeAssemblyService {
    /// Authenticated transport entrypoint. The returned handle stays in the
    /// server process; neither a capability token nor caller-authored authority
    /// is accepted by this interface.
    pub fn attach_external_agent_session(
        &self,
        deployment_id: &str,
        activation_id: &str,
        idempotency_key: &str,
    ) -> Result<ExternalAgentSession, ServiceResponse> {
        if self.agent_mcp_execution_context().is_some() {
            return Err(ServiceResponse::forbidden(
                "external_attach_operator_required",
            ));
        }
        let (_, subject) = self
            .responsible_human_identity()
            .ok_or_else(|| ServiceResponse::forbidden("external_attach_identity_required"))?;
        let key = IdempotencyKey::new(idempotency_key)
            .map_err(|_| ServiceResponse::bad_request("idempotency_key_required"))?;
        let deployment = agent_runner::deployments(&self.runtime())
            .map_err(|_| ServiceResponse::internal_error("external_attach_storage_unavailable"))?
            .into_iter()
            .find(|d| d.deployment_id == deployment_id)
            .ok_or_else(|| ServiceResponse::not_found("agent_deployment"))?;
        let config = execution::authorized_activation_config(
            self,
            &json!({"configId":deployment.execution_config_version_id}),
        )?;
        self.require_object("execution_activation", activation_id)?;
        let activation = self
            .runtime()
            .storage
            .get_json("execution_activations", activation_id)
            .map_err(|_| ServiceResponse::internal_error("external_attach_storage_unavailable"))?
            .ok_or_else(|| ServiceResponse::not_found("execution_activation"))?;
        if deployment.mode == "live" {
            live_authorization::verify_external_delegate(
                self,
                deployment_id,
                &config,
                &activation,
            )?;
        }
        let context = SideEffectContext::new(
            AuthorityContext {
                actor: subject.to_string(),
                surface: "external_agent_attach".into(),
                account_mode: deployment.mode,
            },
            key,
        );
        ExternalAgentSession::attach(self.runtime(), deployment_id, activation_id, &context)
            .map_err(|code| {
                ServiceResponse::conflict_with_details(&code, json!({"failClosed":true}))
            })
    }
}
