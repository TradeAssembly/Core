// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{execution, TradeAssemblyService};
use crate::ports::{
    AuthorityContext, CorePaperRunnerPort, IdempotencyKey, ImmutablePutOutcome, PaperRunnerCommand,
    PaperRunnerOperation, PaperRunnerReceipt, PaperRunnerReceiptState, PortDescriptor, PortKind,
    RunnerBridgeError, RunnerBridgeErrorCode, SideEffectContext, VersionedPort,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const COMMANDS_NS: &str = "core_paper_runner_commands_v1";
const IDEMPOTENCY_NS: &str = "core_paper_runner_idempotency_v1";
const RECEIPTS_NS: &str = "core_paper_runner_receipts_v1";
const ACTIVATIONS_NS: &str = "core_paper_runner_activation_bindings_v1";
const RESULTS_NS: &str = "core_paper_runner_results_v1";

#[derive(Clone)]
pub struct DurableCorePaperRunner {
    service: TradeAssemblyService,
    expected_core_release: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredCommand {
    schema: String,
    command_sha256: String,
    command: PaperRunnerCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IdempotencyBinding {
    schema: String,
    workspace_id: String,
    idempotency_key: String,
    command_id: String,
    command_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostedActivationBinding {
    schema: String,
    workspace_id: String,
    activation_id: String,
    config_id: String,
    strategy_id: String,
    strategy_version_id: String,
    strategy_spec_hash: String,
    capability_graph_revision_id: String,
    capability_graph_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredResult {
    schema: String,
    command_sha256: String,
    result_sha256: String,
    snapshot: Value,
}

impl DurableCorePaperRunner {
    pub fn new(
        service: TradeAssemblyService,
        expected_core_release: impl Into<String>,
    ) -> Result<Self, RunnerBridgeError> {
        let expected_core_release = expected_core_release.into();
        if expected_core_release.trim().is_empty() || expected_core_release.len() > 256 {
            return Err(adapter_error(
                "configured Core release for paper runner is invalid",
            ));
        }
        Ok(Self {
            service,
            expected_core_release,
        })
    }

    fn bind_command(&self, command: &PaperRunnerCommand) -> Result<(), RunnerBridgeError> {
        command.validate(&self.expected_core_release)?;
        let command_sha256 = command.canonical_hash()?;
        let context = command_context(command)?;
        let binding = IdempotencyBinding {
            schema: "tradeassembly.core-paper-runner.idempotency.v1".to_string(),
            workspace_id: command.workspace_id.clone(),
            idempotency_key: command.idempotency_key.as_str().to_string(),
            command_id: command.command_id.clone(),
            command_sha256: command_sha256.clone(),
        };
        self.bind_immutable(
            IDEMPOTENCY_NS,
            &idempotency_key(command),
            serde_json::to_value(binding)
                .map_err(|_| adapter_error("paper runner binding could not be serialized"))?,
            &context,
            "idempotency binding",
        )?;
        let stored = StoredCommand {
            schema: "tradeassembly.core-paper-runner.command.v1".to_string(),
            command_sha256,
            command: command.clone(),
        };
        self.bind_immutable(
            COMMANDS_NS,
            &command_key(&command.workspace_id, &command.command_id),
            serde_json::to_value(stored)
                .map_err(|_| adapter_error("paper runner command could not be serialized"))?,
            &context,
            "command",
        )
    }

    fn validate_pre_admission(
        &self,
        command: &PaperRunnerCommand,
    ) -> Result<(), RunnerBridgeError> {
        command.validate(&self.expected_core_release)?;
        let PaperRunnerOperation::Activate { config_id, .. } = &command.operation else {
            return Ok(());
        };
        if execution::paper_runner_config_snapshot(&self.service, config_id).is_some_and(|config| {
            config["mode"]
                .as_str()
                .is_some_and(|mode| mode.eq_ignore_ascii_case("live"))
        }) {
            return Err(RunnerBridgeError {
                code: RunnerBridgeErrorCode::LiveModeNotAllowed,
                message: "live execution is not allowed through the paper runner".to_string(),
            });
        }
        Ok(())
    }

    fn bind_immutable(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
        context: &SideEffectContext,
        label: &str,
    ) -> Result<(), RunnerBridgeError> {
        let storage = &self.service.runtime().storage;
        if let Some(existing) = storage
            .get_json(namespace, key)
            .map_err(|_| adapter_error(&format!("paper runner {label} could not be read")))?
        {
            return if existing == value {
                Ok(())
            } else {
                Err(conflict())
            };
        }
        match storage.put_json_if_absent(namespace, key, value.clone(), context) {
            Ok(ImmutablePutOutcome::Created) => Ok(()),
            Ok(ImmutablePutOutcome::AlreadyPresent) | Err(_) => {
                let existing = storage
                    .get_json(namespace, key)
                    .map_err(|_| adapter_error(&format!("paper runner {label} could not be read")))?
                    .ok_or_else(|| {
                        adapter_error(&format!("paper runner {label} could not be stored"))
                    })?;
                if existing == value {
                    Ok(())
                } else {
                    Err(conflict())
                }
            }
        }
    }

    fn load_command(
        &self,
        workspace_id: &str,
        command_id: &str,
    ) -> Result<Option<PaperRunnerCommand>, RunnerBridgeError> {
        validate_lookup(workspace_id, command_id)?;
        let Some(value) = self
            .service
            .runtime()
            .storage
            .get_json(COMMANDS_NS, &command_key(workspace_id, command_id))
            .map_err(|_| adapter_error("paper runner command could not be read"))?
        else {
            return Ok(None);
        };
        let stored: StoredCommand = serde_json::from_value(value)
            .map_err(|_| adapter_error("paper runner command record is corrupt"))?;
        stored.command.validate(&self.expected_core_release)?;
        if stored.schema != "tradeassembly.core-paper-runner.command.v1"
            || stored.command.workspace_id != workspace_id
            || stored.command.command_id != command_id
            || stored.command.canonical_hash()? != stored.command_sha256
        {
            return Err(adapter_error(
                "paper runner command record failed integrity validation",
            ));
        }
        Ok(Some(stored.command))
    }

    fn load_receipt(
        &self,
        command: &PaperRunnerCommand,
    ) -> Result<Option<PaperRunnerReceipt>, RunnerBridgeError> {
        let Some(value) = self
            .service
            .runtime()
            .storage
            .get_json(
                RECEIPTS_NS,
                &command_key(&command.workspace_id, &command.command_id),
            )
            .map_err(|_| adapter_error("paper runner receipt could not be read"))?
        else {
            return Ok(None);
        };
        let receipt: PaperRunnerReceipt = serde_json::from_value(value)
            .map_err(|_| adapter_error("paper runner receipt record is corrupt"))?;
        receipt.verify(command, &self.expected_core_release)?;
        Ok(Some(receipt))
    }

    fn persist_receipt(
        &self,
        command: &PaperRunnerCommand,
        receipt: PaperRunnerReceipt,
    ) -> Result<PaperRunnerReceipt, RunnerBridgeError> {
        receipt.verify(command, &self.expected_core_release)?;
        let key = command_key(&command.workspace_id, &command.command_id);
        let context = command_context(command)?;
        let candidate = serde_json::to_value(&receipt)
            .map_err(|_| adapter_error("paper runner receipt could not be serialized"))?;
        match self.service.runtime().storage.put_json_if_absent(
            RECEIPTS_NS,
            &key,
            candidate,
            &context,
        ) {
            Ok(ImmutablePutOutcome::Created) => Ok(receipt),
            Ok(ImmutablePutOutcome::AlreadyPresent) | Err(_) => self
                .load_receipt(command)?
                .ok_or_else(|| adapter_error("paper runner receipt could not be stored")),
        }
    }

    fn drive(&self, command: &PaperRunnerCommand) -> Result<PaperRunnerReceipt, RunnerBridgeError> {
        if let Some(receipt) = self.load_receipt(command)? {
            return Ok(receipt);
        }
        if let Some(snapshot) = self.load_result(command)? {
            return self.persist_receipt(command, completed_receipt(command, &snapshot)?);
        }
        let outcome = match &command.operation {
            PaperRunnerOperation::Activate {
                activation_id,
                config_id,
                strategy_id,
                ..
            } => {
                if let Err(code) = self.verify_activation_preflight(command) {
                    return self.persist_receipt(command, failed_receipt(command, &code)?);
                }
                if execution::paper_runner_activation_snapshot(&self.service, activation_id)
                    .is_some()
                {
                    self.snapshot(command)
                } else {
                    let response = self.service.handle_http_from_source(
                        "core-paper-runner",
                        "POST",
                        "/product/strategy-execution-activations/activate",
                        json!({
                            "activationId": activation_id,
                            "configId": config_id,
                            "strategyId": strategy_id,
                            "idempotencyKey": command.idempotency_key.as_str(),
                            "authorityContext": {
                                "actor": command.authority_ref,
                                "surface": "core-paper-runner",
                                "accountMode": "paper",
                            },
                            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
                        }),
                    );
                    if response.status >= 400 {
                        Err(response_error(&response.body, "paper_activation_failed"))
                    } else {
                        self.snapshot(command)
                    }
                }
            }
            PaperRunnerOperation::Inspect { .. } => self
                .verify_activation_owner(command)
                .and_then(|()| self.snapshot(command)),
            PaperRunnerOperation::Stop { activation_id } => {
                if let Err(code) = self.verify_activation_owner(command) {
                    return self.persist_receipt(command, failed_receipt(command, &code)?);
                }
                if execution::paper_runner_activation_snapshot(&self.service, activation_id)
                    .is_some_and(|snapshot| snapshot["state"].as_str() == Some("stopped"))
                {
                    self.snapshot(command)
                } else {
                    let response = self.service.handle_http_from_source(
                        "core-paper-runner",
                        "POST",
                        "/product/strategy-execution-activations/control",
                        json!({
                            "activationId": activation_id,
                            "action": "stop",
                            "idempotencyKey": command.idempotency_key.as_str(),
                            "controlPlaneCommandId": command.command_id,
                            "authorityContext": {
                                "actor": command.authority_ref,
                                "surface": "core-paper-runner",
                                "accountMode": "paper",
                            },
                        }),
                    );
                    if response.status >= 400
                        || response.body["body"]["status"].as_str() == Some("rejected")
                    {
                        Err(response_error(&response.body, "paper_stop_failed"))
                    } else {
                        self.snapshot(command)
                    }
                }
            }
        };
        let receipt = match outcome {
            Ok(snapshot) => {
                let snapshot = self.persist_result(command, snapshot)?;
                completed_receipt(command, &snapshot)?
            }
            Err(code) => failed_receipt(command, &code)?,
        };
        self.persist_receipt(command, receipt)
    }

    fn load_result(
        &self,
        command: &PaperRunnerCommand,
    ) -> Result<Option<Value>, RunnerBridgeError> {
        let Some(value) = self
            .service
            .runtime()
            .storage
            .get_json(
                RESULTS_NS,
                &command_key(&command.workspace_id, &command.command_id),
            )
            .map_err(|_| adapter_error("paper runner result could not be read"))?
        else {
            return Ok(None);
        };
        let stored: StoredResult = serde_json::from_value(value)
            .map_err(|_| adapter_error("paper runner result record is corrupt"))?;
        let canonical = serde_json_canonicalizer::to_vec(&stored.snapshot)
            .map_err(|_| adapter_error("paper runner result could not be canonicalized"))?;
        let result_sha256 = format!("sha256:{:x}", Sha256::digest(canonical));
        if stored.schema != "tradeassembly.core-paper-runner.result.v1"
            || stored.command_sha256 != command.canonical_hash()?
            || stored.result_sha256 != result_sha256
        {
            return Err(adapter_error(
                "paper runner result record failed integrity validation",
            ));
        }
        Ok(Some(stored.snapshot))
    }

    fn persist_result(
        &self,
        command: &PaperRunnerCommand,
        snapshot: Value,
    ) -> Result<Value, RunnerBridgeError> {
        let canonical = serde_json_canonicalizer::to_vec(&snapshot)
            .map_err(|_| adapter_error("paper runner result could not be canonicalized"))?;
        let stored = StoredResult {
            schema: "tradeassembly.core-paper-runner.result.v1".to_string(),
            command_sha256: command.canonical_hash()?,
            result_sha256: format!("sha256:{:x}", Sha256::digest(canonical)),
            snapshot,
        };
        self.bind_immutable(
            RESULTS_NS,
            &command_key(&command.workspace_id, &command.command_id),
            serde_json::to_value(&stored)
                .map_err(|_| adapter_error("paper runner result could not be serialized"))?,
            &command_context(command)?,
            "result",
        )?;
        self.load_result(command)?
            .ok_or_else(|| adapter_error("paper runner result disappeared"))
    }

    fn verify_activation_preflight(&self, command: &PaperRunnerCommand) -> Result<(), String> {
        let PaperRunnerOperation::Activate {
            activation_id,
            config_id,
            strategy_id,
            strategy_version_id,
            strategy_spec_hash,
            capability_graph_revision_id,
            capability_graph_fingerprint,
        } = &command.operation
        else {
            return Err("paper_activate_operation_required".to_string());
        };
        let config = execution::paper_runner_config_snapshot(&self.service, config_id)
            .ok_or_else(|| "paper_config_not_found".to_string())?;
        if config["ready"].as_bool() != Some(true) {
            return Err("paper_config_not_ready".to_string());
        }
        if config["mode"].as_str() != Some("paper") {
            return Err("paper_mode_required".to_string());
        }
        let matches = config["configId"].as_str() == Some(config_id)
            && config["strategyId"].as_str() == Some(strategy_id)
            && config["strategyVersionId"].as_str() == Some(strategy_version_id)
            && config["strategySpecHash"].as_str() == Some(strategy_spec_hash)
            && config["capabilityGraphRevisionId"].as_str() == Some(capability_graph_revision_id)
            && config["capabilityGraphFingerprint"].as_str() == Some(capability_graph_fingerprint);
        if !matches {
            return Err("immutable_activation_binding_mismatch".to_string());
        }
        let binding = HostedActivationBinding {
            schema: "tradeassembly.core-paper-runner.activation-binding.v1".to_string(),
            workspace_id: command.workspace_id.clone(),
            activation_id: activation_id.clone(),
            config_id: config_id.clone(),
            strategy_id: strategy_id.clone(),
            strategy_version_id: strategy_version_id.clone(),
            strategy_spec_hash: strategy_spec_hash.clone(),
            capability_graph_revision_id: capability_graph_revision_id.clone(),
            capability_graph_fingerprint: capability_graph_fingerprint.clone(),
        };
        let key = activation_key(activation_id);
        let storage = &self.service.runtime().storage;
        if let Some(existing) = storage
            .get_json(ACTIVATIONS_NS, &key)
            .map_err(|_| "activation_binding_unavailable".to_string())?
        {
            return if existing
                == serde_json::to_value(&binding)
                    .map_err(|_| "activation_binding_invalid".to_string())?
            {
                Ok(())
            } else {
                Err("activation_ownership_conflict".to_string())
            };
        }
        if execution::paper_runner_activation_snapshot(&self.service, activation_id).is_some() {
            return Err("activation_not_owned_by_runner".to_string());
        }
        self.bind_immutable(
            ACTIVATIONS_NS,
            &key,
            serde_json::to_value(binding).map_err(|_| "activation_binding_invalid".to_string())?,
            &command_context(command).map_err(|_| "activation_binding_invalid".to_string())?,
            "activation binding",
        )
        .map_err(|error| safe_failure_code(&format!("{:?}", error.code)))
    }

    fn verify_activation_owner(&self, command: &PaperRunnerCommand) -> Result<(), String> {
        let activation_id = command.operation.activation_id();
        let value = self
            .service
            .runtime()
            .storage
            .get_json(ACTIVATIONS_NS, &activation_key(activation_id))
            .map_err(|_| "activation_binding_unavailable".to_string())?
            .ok_or_else(|| "activation_not_owned_by_runner".to_string())?;
        let binding: HostedActivationBinding =
            serde_json::from_value(value).map_err(|_| "activation_binding_corrupt".to_string())?;
        if binding.schema != "tradeassembly.core-paper-runner.activation-binding.v1"
            || binding.activation_id != activation_id
            || binding.workspace_id != command.workspace_id
        {
            return Err("activation_ownership_conflict".to_string());
        }
        Ok(())
    }

    fn snapshot(&self, command: &PaperRunnerCommand) -> Result<Value, String> {
        let snapshot = execution::paper_runner_activation_snapshot(
            &self.service,
            command.operation.activation_id(),
        )
        .ok_or_else(|| "activation_not_found_or_inconsistent".to_string())?;
        if snapshot["mode"].as_str() != Some("paper") {
            return Err("paper_mode_required".to_string());
        }
        if let PaperRunnerOperation::Activate {
            config_id,
            strategy_id,
            strategy_version_id,
            strategy_spec_hash,
            capability_graph_revision_id,
            capability_graph_fingerprint,
            ..
        } = &command.operation
        {
            let matches = snapshot["configId"].as_str() == Some(config_id)
                && snapshot["strategyId"].as_str() == Some(strategy_id)
                && snapshot["strategyVersionId"].as_str() == Some(strategy_version_id)
                && snapshot["strategySpecHash"].as_str() == Some(strategy_spec_hash)
                && snapshot["capabilityGraphRevisionId"].as_str()
                    == Some(capability_graph_revision_id)
                && snapshot["capabilityGraphFingerprint"].as_str()
                    == Some(capability_graph_fingerprint);
            if !matches {
                return Err("immutable_activation_binding_mismatch".to_string());
            }
        }
        if matches!(command.operation, PaperRunnerOperation::Stop { .. })
            && snapshot["state"].as_str() != Some("stopped")
        {
            return Err("paper_stop_not_durable".to_string());
        }
        Ok(snapshot)
    }
}

impl VersionedPort for DurableCorePaperRunner {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::CoreRunnerBridge, "durable-core-paper-runner")
                .for_profiles(&["local", "self_hosted", "serverless"])
                .with_capabilities(&[
                    "hosted_paper_activate",
                    "hosted_paper_inspect",
                    "hosted_paper_stop",
                ]),
        ]
    }
}

impl CorePaperRunnerPort for DurableCorePaperRunner {
    fn dispatch(
        &self,
        command: &PaperRunnerCommand,
    ) -> Result<PaperRunnerReceipt, RunnerBridgeError> {
        self.validate_pre_admission(command)?;
        self.bind_command(command)?;
        self.drive(command)
    }

    fn receipt(
        &self,
        workspace_id: &str,
        command_id: &str,
    ) -> Result<Option<PaperRunnerReceipt>, RunnerBridgeError> {
        let Some(command) = self.load_command(workspace_id, command_id)? else {
            return Ok(None);
        };
        self.drive(&command).map(Some)
    }
}

fn completed_receipt(
    command: &PaperRunnerCommand,
    snapshot: &Value,
) -> Result<PaperRunnerReceipt, RunnerBridgeError> {
    let result = serde_json_canonicalizer::to_vec(snapshot)
        .map_err(|_| adapter_error("paper runner snapshot could not be canonicalized"))?;
    let receipt = PaperRunnerReceipt {
        schema: crate::ports::CORE_PAPER_RUNNER_SCHEMA.to_string(),
        bridge_version: crate::ports::CORE_PAPER_RUNNER_VERSION.to_string(),
        receipt_id: receipt_id(command),
        workspace_id: command.workspace_id.clone(),
        command_id: command.command_id.clone(),
        idempotency_key: command.idempotency_key.clone(),
        operation: command.operation.kind(),
        activation_id: command.operation.activation_id().to_string(),
        core_release: command.core_release.clone(),
        command_sha256: command.canonical_hash()?,
        state: PaperRunnerReceiptState::Completed,
        activation_state: snapshot["state"].as_str().map(str::to_string),
        result_sha256: Some(format!("sha256:{:x}", Sha256::digest(result))),
        evidence_refs: vec![
            format!(
                "tradeassembly://execution/{}/activation",
                command.operation.activation_id()
            ),
            format!(
                "tradeassembly://execution/{}/journal",
                command.operation.activation_id()
            ),
        ],
        failure_code: None,
        recorded_at_ms: command.submitted_at_ms,
    };
    receipt.verify(command, &command.core_release)?;
    Ok(receipt)
}

fn failed_receipt(
    command: &PaperRunnerCommand,
    failure_code: &str,
) -> Result<PaperRunnerReceipt, RunnerBridgeError> {
    let receipt = PaperRunnerReceipt {
        schema: crate::ports::CORE_PAPER_RUNNER_SCHEMA.to_string(),
        bridge_version: crate::ports::CORE_PAPER_RUNNER_VERSION.to_string(),
        receipt_id: receipt_id(command),
        workspace_id: command.workspace_id.clone(),
        command_id: command.command_id.clone(),
        idempotency_key: command.idempotency_key.clone(),
        operation: command.operation.kind(),
        activation_id: command.operation.activation_id().to_string(),
        core_release: command.core_release.clone(),
        command_sha256: command.canonical_hash()?,
        state: PaperRunnerReceiptState::Failed,
        activation_state: None,
        result_sha256: None,
        evidence_refs: Vec::new(),
        failure_code: Some(safe_failure_code(failure_code)),
        recorded_at_ms: command.submitted_at_ms,
    };
    receipt.verify(command, &command.core_release)?;
    Ok(receipt)
}

fn response_error(body: &Value, fallback: &str) -> String {
    body.pointer("/error/code")
        .or_else(|| body.pointer("/body/error/code"))
        .and_then(Value::as_str)
        .map(safe_failure_code)
        .unwrap_or_else(|| fallback.to_string())
}

fn safe_failure_code(value: &str) -> String {
    let sanitized = value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        .take(128)
        .collect::<String>();
    if sanitized.is_empty() {
        "paper_runner_failed".to_string()
    } else {
        sanitized
    }
}

fn command_context(command: &PaperRunnerCommand) -> Result<SideEffectContext, RunnerBridgeError> {
    Ok(SideEffectContext::new(
        AuthorityContext {
            actor: command.authority_ref.clone(),
            surface: "core-paper-runner".to_string(),
            account_mode: "paper".to_string(),
        },
        IdempotencyKey::new(format!(
            "core-paper-runner:{}",
            stable_suffix(&command.canonical_hash()?)
        ))
        .map_err(|_| adapter_error("paper runner persistence idempotency is invalid"))?,
    ))
}

fn command_key(workspace_id: &str, command_id: &str) -> String {
    stable_key("command", workspace_id, command_id)
}

fn idempotency_key(command: &PaperRunnerCommand) -> String {
    stable_key(
        "idempotency",
        &command.workspace_id,
        command.idempotency_key.as_str(),
    )
}

fn activation_key(activation_id: &str) -> String {
    format!("activation-{:x}", Sha256::digest(activation_id))
}

fn stable_key(kind: &str, left: &str, right: &str) -> String {
    format!("{kind}-{:x}", Sha256::digest(format!("{left}\0{right}")))
}

fn stable_suffix(value: &str) -> String {
    format!("{:x}", Sha256::digest(value))[..16].to_string()
}

fn receipt_id(command: &PaperRunnerCommand) -> String {
    format!(
        "paper-receipt-{}",
        stable_suffix(&format!(
            "{}:{}:{}",
            command.workspace_id, command.command_id, command.core_release
        ))
    )
}

fn validate_lookup(workspace_id: &str, command_id: &str) -> Result<(), RunnerBridgeError> {
    if workspace_id.trim().is_empty()
        || command_id.trim().is_empty()
        || workspace_id.len() > 256
        || command_id.len() > 256
    {
        return Err(RunnerBridgeError {
            code: RunnerBridgeErrorCode::MalformedCommand,
            message: "paper runner receipt lookup is invalid".to_string(),
        });
    }
    Ok(())
}

fn conflict() -> RunnerBridgeError {
    RunnerBridgeError {
        code: RunnerBridgeErrorCode::IdempotencyConflict,
        message: "paper runner command identity was reused".to_string(),
    }
}

fn adapter_error(message: &str) -> RunnerBridgeError {
    RunnerBridgeError {
        code: RunnerBridgeErrorCode::AdapterFailure,
        message: message.to_string(),
    }
}
