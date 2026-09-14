// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::TradeAssemblyService;
use crate::agent_runner::deployment_binding_digest;
use crate::ports::{
    ClockPort, ConnectedRunnerError, InstallationSignerPort, ReachFinishRequest, ReachNodeRequest,
    ReachNodeScope, ReachOperation, ReachPoll, ReachRecord, ReachState, ReachTransitionRequest,
    ReachTransportPort, RunnerEntropyPort,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct ConnectedReachConsumer {
    transport: Arc<dyn ReachTransportPort>,
    signer: Arc<dyn InstallationSignerPort>,
    service: TradeAssemblyService,
    node: ReachNodeScope,
    clock: Arc<dyn ClockPort>,
    entropy: Arc<dyn RunnerEntropyPort>,
}

impl ConnectedReachConsumer {
    pub fn new(
        transport: Arc<dyn ReachTransportPort>,
        signer: Arc<dyn InstallationSignerPort>,
        service: TradeAssemblyService,
        node: ReachNodeScope,
        clock: Arc<dyn ClockPort>,
        entropy: Arc<dyn RunnerEntropyPort>,
    ) -> Result<Self, ConnectedRunnerError> {
        node.validate()?;
        if signer.installation_id() != node.node_id {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        Ok(Self {
            transport,
            signer,
            service,
            node,
            clock,
            entropy,
        })
    }

    pub fn poll_once(&self) -> Result<ReachPoll, ConnectedRunnerError> {
        let claim = self.signed_request(
            self.entropy.opaque_id("reach-claim-request")?,
            "claim",
            None,
        )?;
        let Some(mut record) = self.transport.claim(&claim)? else {
            return Ok(ReachPoll::Idle);
        };
        record.validate_for(&self.node, self.clock.now_ms())?;
        if !matches!(record.state, ReachState::Delivered | ReachState::Accepted) {
            return Err(ConnectedRunnerError::invalid_contract());
        }

        if record.state == ReachState::Delivered {
            let request = self.signed_request(
                record.command.request_id.clone(),
                "accept",
                Some(&record.command_digest),
            )?;
            record = self.transport.accept(&ReachTransitionRequest {
                request,
                command_digest: record.command_digest.clone(),
            })?;
            record.validate_for(&self.node, self.clock.now_ms())?;
            if record.state != ReachState::Accepted {
                return Err(ConnectedRunnerError::invalid_contract());
            }
        }

        let (state, evidence_ref) = self.execute(&record)?;
        let operation = if state == ReachState::Completed {
            "complete"
        } else {
            "reject"
        };
        let request = self.signed_request(
            record.command.request_id.clone(),
            operation,
            Some(&record.command_digest),
        )?;
        let finished = self.transport.finish(&ReachFinishRequest {
            request,
            command_digest: record.command_digest.clone(),
            state,
            evidence_ref: evidence_ref.clone(),
        })?;
        finished.validate_for(&self.node, self.clock.now_ms())?;
        if finished.state != state
            || finished.command_digest != record.command_digest
            || finished
                .receipt
                .as_ref()
                .is_none_or(|receipt| receipt.evidence_ref != evidence_ref)
        {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        Ok(ReachPoll::Completed {
            request_id: record.command.request_id,
            state,
            evidence_ref,
        })
    }

    fn signed_request(
        &self,
        request_id: String,
        operation: &str,
        digest: Option<&str>,
    ) -> Result<ReachNodeRequest, ConnectedRunnerError> {
        let mut request = ReachNodeRequest {
            node: self.node.clone(),
            request_id,
            nonce: self.entropy.opaque_id("reach-nonce")?,
            issued_at_ms: self.clock.now_ms(),
            signature: String::new(),
        };
        request.signature = self
            .signer
            .sign(&request.signing_payload(operation, digest)?)?;
        Ok(request)
    }

    fn execute(&self, record: &ReachRecord) -> Result<(ReachState, String), ConnectedRunnerError> {
        let inspect_response = self.service.call_mcp_tool(
            "studio.deployment.inspect",
            json!({"deployment_id": record.command.deployment_id}),
        );
        let inspect = mcp_payload(inspect_response);
        let deployments = inspect["deployments"].as_array();
        let deployment = deployments
            .and_then(|values| values.first())
            .filter(|_| deployments.is_some_and(|values| values.len() == 1));
        let Some(deployment) = deployment else {
            return self.rejected_evidence(record, &inspect);
        };
        if deployment["deploymentId"] != record.command.deployment_id
            || deployment["bindingDigest"] != record.command.deployment_version
        {
            return self.rejected_evidence(record, &json!({"code":"binding_mismatch"}));
        }
        let Some(mode) = deployment["mode"].as_str() else {
            return self.rejected_evidence(record, &json!({"code":"mode_missing"}));
        };
        if !matches!(mode, "paper" | "live") {
            return self.rejected_evidence(record, &json!({"code":"mode_invalid"}));
        }

        let result = match record.command.operation {
            ReachOperation::Inspect => inspect,
            ReachOperation::Activate | ReachOperation::Stop => {
                let tool = if record.command.operation == ReachOperation::Activate {
                    "studio.deployment.start"
                } else {
                    "studio.deployment.stop"
                };
                mcp_payload(self.service.call_mcp_tool(
                    tool,
                    json!({
                        "deployment_id": record.command.deployment_id,
                        "idempotency_key": record.command.idempotency_key,
                        "authority_context": {
                            "actor": record.command.actor_ref,
                            "surface": "relay_reach",
                            "accountMode": mode,
                        },
                        "reachRequestId": record.command.request_id,
                        "reachActivationId": record.command.activation_id,
                        "reachDeploymentVersion": record.command.deployment_version,
                        "reachAuthorityRef": record.command.authority_ref,
                        "reachCommandDigest": record.command_digest,
                    }),
                ))
            }
        };
        let state = if result["ok"] == Value::Bool(true) {
            ReachState::Completed
        } else {
            ReachState::Rejected
        };
        Ok((state, evidence_ref(record, &result)?))
    }

    fn rejected_evidence(
        &self,
        record: &ReachRecord,
        result: &Value,
    ) -> Result<(ReachState, String), ConnectedRunnerError> {
        Ok((ReachState::Rejected, evidence_ref(record, result)?))
    }
}

fn mcp_payload(response: Value) -> Value {
    response
        .get("structuredContent")
        .cloned()
        .unwrap_or(response)
}

fn evidence_ref(record: &ReachRecord, result: &Value) -> Result<String, ConnectedRunnerError> {
    let bytes = serde_json_canonicalizer::to_vec(&json!({
        "schema": "tradeassembly.core.reach-evidence/v1",
        "commandDigest": record.command_digest,
        "operation": record.command.operation,
        "result": result,
    }))
    .map_err(|_| ConnectedRunnerError::core_failure())?;
    Ok(format!(
        "tradeassembly://reach-evidence/{}/sha256:{:x}",
        record.command.request_id,
        Sha256::digest(bytes)
    ))
}

pub fn local_deployment_version(
    service: &TradeAssemblyService,
    deployment_id: &str,
) -> Option<String> {
    crate::agent_runner::deployments(&service.runtime())
        .ok()?
        .into_iter()
        .find(|deployment| deployment.deployment_id == deployment_id)
        .map(|deployment| deployment_binding_digest(&deployment))
}
