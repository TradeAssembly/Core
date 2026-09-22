//! Trusted runtime primitive for an externally owned MCP connection.
//! The service facade must authenticate ownership/mandate before attachment.
//! Possessing deployment IDs is never sufficient transport authorization.

use super::*;
use crate::ports::{ComparePutOutcome, StorageExpectation, StorageWrite};

/// Process-held authority. No bearer token is exposed or serialized.
pub struct ExternalAgentSession {
    runtime: std::sync::Arc<ServiceRuntime>,
    capability: AgentMcpExecutionCapability,
    lease: LeaseClaim,
    deployment_id: String,
    run_id: String,
    closed: bool,
}

impl ExternalAgentSession {
    /// Called only after the owning service has authorized this exact activation.
    /// This primitive does not grant a mandate or change activation readiness.
    pub fn attach(
        runtime: std::sync::Arc<ServiceRuntime>,
        deployment_id: &str,
        activation_id: &str,
        side_effect: &SideEffectContext,
    ) -> Result<Self, String> {
        let deployment_record = required(&runtime, DEPLOYMENTS_NS, deployment_id)?;
        let deployment: AgentDeployment =
            serde_json::from_value(deployment_record["deployment"].clone())
                .map_err(|_| "agent_deployment_invalid")?;
        let config = required(
            &runtime,
            "execution_configs",
            &deployment.execution_config_version_id,
        )?;
        let activation = required(&runtime, "execution_activations", activation_id)?;
        if deployment.executor != AgentExecutor::ExternalClient
            || deployment.desired_state != "active"
            || config["orchestrator"] != "external_agent"
            || activation["orchestrator"] != "external_agent"
            || activation["state"] != "active"
            || activation["activationId"] != activation_id
            || activation["configId"] != deployment.execution_config_version_id
            || activation["mode"] != deployment.mode
            || config["mode"] != deployment.mode
            || side_effect.authority.account_mode != deployment.mode
        {
            return Err("external_agent_binding_invalid".into());
        }
        for field in [
            "strategyVersionId",
            "strategySpecHash",
            "capabilityGraphRevisionId",
            "accountRef",
            "riskLimits",
        ] {
            if activation[field].is_null() || activation[field] != config[field] {
                return Err("external_agent_binding_invalid".into());
            }
        }
        let prior = runtime.storage.get_json(ACTIVE_RUNS_NS, deployment_id)?;
        if prior
            .as_ref()
            .is_some_and(|p| !matches!(p["state"].as_str(), Some("completed" | "reconciled")))
        {
            return Err("agent_run_pending_reconcile".into());
        }
        let now = runtime.clock.trusted_now_ms()?;
        let capability = AgentMcpExecutionCapability::issue();
        let digest = capability.digest();
        let run_id = format!("external_{}", &digest[..24]);
        let lease = runtime
            .leases
            .acquire(
                &format!("agent-deployment:{deployment_id}"),
                &run_id,
                now,
                DEFAULT_LEASE_MS,
            )?
            .ok_or("agent_run_lease_held")?;
        let binding_digest = deployment_binding_digest(&deployment);
        let record = AgentMcpCapabilityRecord {
            schema_version: SCHEMA_VERSION.into(),
            deployment_id: deployment_id.into(),
            run_id: run_id.clone(),
            binding_digest: binding_digest.clone(),
            mode: deployment.mode.clone(),
            studio_tool_allowlist: canonical_allowlist(&deployment.studio_tool_allowlist),
            capability_digest: digest.clone(),
        };
        let run = json!({"schemaVersion":SCHEMA_VERSION,"kind":"AgentRun","executor":"external_client",
            "activationId":activation_id,"bindingDigest":binding_digest,"mcpCapabilityDigest":digest,
            "run":{"runId":run_id,"deploymentId":deployment_id,"triggerId":"external_attach",
                "state":"running","leaseFence":lease.fencing_token,"deploymentBindingDigest":binding_digest}});
        let result = runtime.storage.put_json_batch(
            &[
                StorageWrite::new(RUNS_NS, &run_id, run, side_effect.clone()),
                StorageWrite::new(
                    ACTIVE_RUNS_NS,
                    deployment_id,
                    active_run_record(&run_id, "running", &lease, now),
                    side_effect.clone(),
                ),
                StorageWrite::new(
                    MCP_CAPABILITIES_NS,
                    &digest,
                    serde_json::to_value(record).map_err(|_| "agent_capability_invalid")?,
                    side_effect.clone(),
                ),
            ],
            &[
                StorageExpectation::new(DEPLOYMENTS_NS, deployment_id, Some(deployment_record)),
                StorageExpectation::new(
                    "execution_configs",
                    &deployment.execution_config_version_id,
                    Some(config),
                ),
                StorageExpectation::new("execution_activations", activation_id, Some(activation)),
                StorageExpectation::new(ACTIVE_RUNS_NS, deployment_id, prior),
                StorageExpectation::new(RUNS_NS, &run_id, None),
                StorageExpectation::new(MCP_CAPABILITIES_NS, &digest, None),
            ],
        );
        if !matches!(result, Ok(ComparePutOutcome::Updated)) {
            let _ = runtime.leases.release(&lease);
            return Err("external_agent_attach_conflict".into());
        }
        let session = Self {
            runtime,
            capability,
            lease,
            deployment_id: deployment_id.into(),
            run_id,
            closed: false,
        };
        session.verified_context()?;
        Ok(session)
    }

    pub fn verified_context(&self) -> Result<VerifiedAgentMcpExecutionContext, String> {
        if self.closed {
            return Err("external_agent_session_closed".into());
        }
        let context = self.capability.verified_context(&self.runtime)?;
        context.current_lease(
            self.runtime.storage.as_ref(),
            self.runtime.clock.as_ref(),
            self.runtime.leases.as_ref(),
        )?;
        Ok(context)
    }

    pub fn renew(&mut self) -> Result<(), String> {
        let result = self.renew_current();
        if result.is_err() {
            let _ = self.close();
        }
        result
    }

    fn renew_current(&mut self) -> Result<(), String> {
        self.verified_context()?;
        let prior = required(&self.runtime, ACTIVE_RUNS_NS, &self.deployment_id)?;
        let now = self.runtime.clock.trusted_now_ms()?;
        self.lease = self
            .runtime
            .leases
            .renew(&self.lease, now, DEFAULT_LEASE_MS)?;
        let result = self.runtime.storage.put_json_batch(
            &[StorageWrite::new(
                ACTIVE_RUNS_NS,
                &self.deployment_id,
                active_run_record(&self.run_id, "running", &self.lease, now),
                context(&format!("external-heartbeat:{}:{now}", self.run_id)),
            )],
            &[StorageExpectation::new(
                ACTIVE_RUNS_NS,
                &self.deployment_id,
                Some(prior),
            )],
        )?;
        if result != ComparePutOutcome::Updated {
            return Err("external_agent_renew_conflict".into());
        }
        self.verified_context().map(|_| ())
    }

    /// Detach revokes authority, retaining uncertain outcomes for reconciliation.
    /// It never labels a disconnected agent's broker activity completed.
    pub fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        let result = (|| {
            let active = required(&self.runtime, ACTIVE_RUNS_NS, &self.deployment_id)?;
            if active["runId"] != self.run_id || active["leaseFence"] != self.lease.fencing_token {
                return Err("agent_run_lease_invalid".into());
            }
            let run = required(&self.runtime, RUNS_NS, &self.run_id)?;
            let mut ended = run.clone();
            ended["run"]["state"] = json!("pending_reconcile");
            let mut pointer = active.clone();
            pointer["state"] = json!("pending_reconcile");
            let ctx = context(&format!("external-detach:{}", self.run_id));
            match self.runtime.storage.put_json_batch(
                &[
                    StorageWrite::new(RUNS_NS, &self.run_id, ended, ctx.clone()),
                    StorageWrite::new(ACTIVE_RUNS_NS, &self.deployment_id, pointer, ctx),
                ],
                &[
                    StorageExpectation::new(RUNS_NS, &self.run_id, Some(run)),
                    StorageExpectation::new(ACTIVE_RUNS_NS, &self.deployment_id, Some(active)),
                ],
            )? {
                ComparePutOutcome::Updated => Ok(()),
                ComparePutOutcome::Conflict => Err("external_agent_detach_conflict".into()),
            }
        })();
        self.closed = true;
        let release = self.runtime.leases.release(&self.lease);
        result.and(release)
    }
}

impl Drop for ExternalAgentSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn required(runtime: &ServiceRuntime, namespace: &str, key: &str) -> Result<Value, String> {
    runtime
        .storage
        .get_json(namespace, key)?
        .ok_or_else(|| "external_agent_binding_missing".into())
}

/// Only the recovery path calls this after acquiring the deployment lease.
/// A crashed connection cannot run Drop; preserve its run before owner recovery.
pub(super) fn quarantine_abandoned(
    runtime: &ServiceRuntime,
    deployment_id: &str,
    ctx: &SideEffectContext,
) -> Result<(), String> {
    let Some(active) = runtime.storage.get_json(ACTIVE_RUNS_NS, deployment_id)? else {
        return Ok(());
    };
    if active["state"] != "running" {
        return Ok(());
    }
    let run_id = active["runId"]
        .as_str()
        .ok_or("external_agent_binding_invalid")?;
    let run = required(runtime, RUNS_NS, run_id)?;
    if run["executor"] != "external_client" || run["run"]["deploymentId"] != deployment_id {
        return Err("external_agent_binding_invalid".into());
    }
    let mut ended = run.clone();
    ended["run"]["state"] = json!("pending_reconcile");
    let mut pointer = active.clone();
    pointer["state"] = json!("pending_reconcile");
    match runtime.storage.put_json_batch(
        &[
            StorageWrite::new(RUNS_NS, run_id, ended, ctx.clone()),
            StorageWrite::new(ACTIVE_RUNS_NS, deployment_id, pointer, ctx.clone()),
        ],
        &[
            StorageExpectation::new(RUNS_NS, run_id, Some(run)),
            StorageExpectation::new(ACTIVE_RUNS_NS, deployment_id, Some(active.clone())),
        ],
    )? {
        ComparePutOutcome::Updated => Ok(()),
        ComparePutOutcome::Conflict => Err("external_agent_recovery_conflict".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::TradeAssemblyService;

    // Component fixture only: complete customer proof must use real activation.
    fn fixture() -> (tempfile::TempDir, std::sync::Arc<ServiceRuntime>) {
        let temp = tempfile::tempdir().unwrap();
        let service =
            TradeAssemblyService::test_local(temp.path().join("session.db").to_str().unwrap());
        let runtime = service.runtime();
        let mut deployment = super::super::tests::deployment();
        deployment.executor = AgentExecutor::ExternalClient;
        deployment.deployment_id = "external".into();
        put_deployment(&runtime, &deployment).unwrap();
        let config = json!({"configId":"config-v1","orchestrator":"external_agent","mode":"paper",
            "strategyVersionId":"strategy-v1","strategySpecHash":"spec-hash","capabilityGraphRevisionId":"cap-v1",
            "accountRef":"controlled-paper","riskLimits":{"maxOrderNotional":100}});
        let mut activation = config.clone();
        activation["activationId"] = json!("activation-1");
        activation["state"] = json!("active");
        runtime
            .storage
            .put_json(
                "execution_configs",
                "config-v1",
                config,
                &context("fixture-config"),
            )
            .unwrap();
        runtime
            .storage
            .put_json(
                "execution_activations",
                "activation-1",
                activation,
                &context("fixture-activation"),
            )
            .unwrap();
        (temp, runtime)
    }

    #[test]
    fn session_is_exclusive_renewable_and_quarantined_on_disconnect() {
        let (_temp, runtime) = fixture();
        let mut session = ExternalAgentSession::attach(
            runtime.clone(),
            "external",
            "activation-1",
            &context("attach"),
        )
        .unwrap();
        let identity = session.verified_context().unwrap();
        assert!(ExternalAgentSession::attach(
            runtime.clone(),
            "external",
            "activation-1",
            &context("another")
        )
        .is_err());
        session.renew().unwrap();
        assert_eq!(identity, session.verified_context().unwrap());
        assert!(runtime.storage.list_json(SCHEDULES_NS).unwrap().is_empty());
        session.close().unwrap();
        session.close().unwrap();
        assert!(identity
            .current_lease(
                runtime.storage.as_ref(),
                runtime.clock.as_ref(),
                runtime.leases.as_ref()
            )
            .is_err());
        assert_eq!(
            required(&runtime, ACTIVE_RUNS_NS, "external").unwrap()["state"],
            "pending_reconcile"
        );
        assert!(ExternalAgentSession::attach(
            runtime.clone(),
            "external",
            "activation-1",
            &context("reconnect")
        )
        .is_err());
        recover_pending_run(
            &runtime,
            "external",
            runtime.clock.trusted_now_ms().unwrap(),
        )
        .unwrap();
        assert!(runtime.storage.list_json(SCHEDULES_NS).unwrap().is_empty());
        let replacement = ExternalAgentSession::attach(
            runtime.clone(),
            "external",
            "activation-1",
            &context("reconciled-attach"),
        )
        .unwrap();
        assert_ne!(identity, replacement.verified_context().unwrap());
        drop(replacement);
        assert_eq!(
            required(&runtime, ACTIVE_RUNS_NS, "external").unwrap()["state"],
            "pending_reconcile"
        );
    }

    #[test]
    fn abandoned_connection_requires_exclusive_operator_recovery() {
        let (_temp, runtime) = fixture();
        let mut session = ExternalAgentSession::attach(
            runtime.clone(),
            "external",
            "activation-1",
            &context("attach"),
        )
        .unwrap();
        assert!(recover_pending_run(
            &runtime,
            "external",
            runtime.clock.trusted_now_ms().unwrap()
        )
        .is_err());
        // Model the lease expiring without Drop: no clean-disconnect transition.
        runtime.leases.release(&session.lease).unwrap();
        session.closed = true;
        drop(session);
        assert_eq!(
            required(&runtime, ACTIVE_RUNS_NS, "external").unwrap()["state"],
            "running"
        );
        assert!(ExternalAgentSession::attach(
            runtime.clone(),
            "external",
            "activation-1",
            &context("crash-reconnect")
        )
        .is_err());
        recover_pending_run(
            &runtime,
            "external",
            runtime.clock.trusted_now_ms().unwrap(),
        )
        .unwrap();
        let new_session = ExternalAgentSession::attach(
            runtime.clone(),
            "external",
            "activation-1",
            &context("recovered"),
        )
        .unwrap();
        new_session.verified_context().unwrap();
        assert!(runtime.storage.list_json(SCHEDULES_NS).unwrap().is_empty());
    }

    #[test]
    fn binding_changes_and_replaced_leases_fail_closed() {
        let (_temp, runtime) = fixture();
        let mut activation = required(&runtime, "execution_activations", "activation-1").unwrap();
        activation["riskLimits"] = json!({"maxOrderNotional":999});
        runtime
            .storage
            .put_json(
                "execution_activations",
                "activation-1",
                activation.clone(),
                &context("mismatch"),
            )
            .unwrap();
        assert!(ExternalAgentSession::attach(
            runtime.clone(),
            "external",
            "activation-1",
            &context("denied")
        )
        .is_err());
        assert!(runtime.storage.list_json(RUNS_NS).unwrap().is_empty());
        activation["riskLimits"] = json!({"maxOrderNotional":100});
        runtime
            .storage
            .put_json(
                "execution_activations",
                "activation-1",
                activation,
                &context("restore"),
            )
            .unwrap();
        let mut session = ExternalAgentSession::attach(
            runtime.clone(),
            "external",
            "activation-1",
            &context("attach"),
        )
        .unwrap();
        runtime.leases.release(&session.lease).unwrap();
        let replacement = runtime
            .leases
            .acquire(
                &session.lease.resource,
                "replacement",
                runtime.clock.trusted_now_ms().unwrap(),
                DEFAULT_LEASE_MS,
            )
            .unwrap()
            .unwrap();
        assert!(session.verified_context().is_err());
        assert!(session.renew().is_err());
        drop(session);
        assert_eq!(
            runtime
                .leases
                .current(&replacement.resource)
                .unwrap()
                .unwrap(),
            replacement
        );
    }
}
