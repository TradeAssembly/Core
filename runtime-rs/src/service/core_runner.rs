// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{backtest_lifecycle, TradeAssemblyService};
use crate::backtest_contracts::BacktestRunRecord;
use crate::domain::BacktestRunState;
use crate::ports::{
    AuthorityContext, ComparePutOutcome, CoreRunnerBridgePort, IdempotencyKey, ImmutablePutOutcome,
    PortDescriptor, PortKind, RunnerBridgeCommand, RunnerBridgeError, RunnerBridgeErrorCode,
    RunnerBridgeReceipt, RunnerBridgeReceiptState, SideEffectContext, VersionedPort,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const COMMANDS_NS: &str = "core_runner_bridge_commands_v1";
const IDEMPOTENCY_NS: &str = "core_runner_bridge_idempotency_v1";
const RECEIPTS_NS: &str = "core_runner_bridge_receipts_v1";

#[derive(Clone)]
pub struct DurableCoreRunnerBridge {
    service: TradeAssemblyService,
    expected_core_release: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredCommand {
    schema: String,
    command_sha256: String,
    command: RunnerBridgeCommand,
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

impl DurableCoreRunnerBridge {
    pub fn new(
        service: TradeAssemblyService,
        expected_core_release: impl Into<String>,
    ) -> Result<Self, RunnerBridgeError> {
        let expected_core_release = expected_core_release.into();
        if expected_core_release.trim().is_empty() || expected_core_release.len() > 256 {
            return Err(adapter_error("configured Core release is invalid"));
        }
        Ok(Self {
            service,
            expected_core_release,
        })
    }

    fn bind_command(&self, command: &RunnerBridgeCommand) -> Result<(), RunnerBridgeError> {
        command.validate(&self.expected_core_release)?;
        let command_sha256 = command.canonical_hash()?;
        let context = command_context(command)?;
        let binding = IdempotencyBinding {
            schema: "tradeassembly.core-runner-bridge.idempotency.v1".to_string(),
            workspace_id: command.workspace_id.clone(),
            idempotency_key: command.idempotency_key.as_str().to_string(),
            command_id: command.command_id.clone(),
            command_sha256: command_sha256.clone(),
        };
        let binding_value = serde_json::to_value(&binding).map_err(|_| {
            adapter_error("runner bridge idempotency binding could not be serialized")
        })?;
        let binding_key = idempotency_key(command);
        self.bind_immutable(
            IDEMPOTENCY_NS,
            &binding_key,
            binding_value,
            &context,
            "idempotency binding",
        )?;

        let stored = StoredCommand {
            schema: "tradeassembly.core-runner-bridge.command.v1".to_string(),
            command_sha256,
            command: command.clone(),
        };
        let stored_value = serde_json::to_value(&stored)
            .map_err(|_| adapter_error("runner bridge command could not be serialized"))?;
        let command_key = command_key(&command.workspace_id, &command.command_id);
        self.bind_immutable(COMMANDS_NS, &command_key, stored_value, &context, "command")
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
            .map_err(|_| adapter_error(&format!("runner bridge {label} could not be read")))?
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
                    .map_err(|_| {
                        adapter_error(&format!("runner bridge {label} could not be read"))
                    })?
                    .ok_or_else(|| {
                        adapter_error(&format!("runner bridge {label} could not be stored"))
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
    ) -> Result<Option<RunnerBridgeCommand>, RunnerBridgeError> {
        validate_lookup(workspace_id, command_id)?;
        let Some(value) = self
            .service
            .runtime()
            .storage
            .get_json(COMMANDS_NS, &command_key(workspace_id, command_id))
            .map_err(|_| adapter_error("runner bridge command could not be read"))?
        else {
            return Ok(None);
        };
        let stored: StoredCommand = serde_json::from_value(value)
            .map_err(|_| adapter_error("runner bridge command record is corrupt"))?;
        stored.command.validate(&self.expected_core_release)?;
        if stored.schema != "tradeassembly.core-runner-bridge.command.v1"
            || stored.command.workspace_id != workspace_id
            || stored.command.command_id != command_id
            || stored.command.canonical_hash()? != stored.command_sha256
        {
            return Err(adapter_error(
                "runner bridge command record failed integrity validation",
            ));
        }
        Ok(Some(stored.command))
    }

    fn load_bound_run(
        &self,
        command: &RunnerBridgeCommand,
    ) -> Result<Option<BacktestRunRecord>, RunnerBridgeError> {
        let run_id = &command.backtest_manifest.content.run_id;
        let runtime = self.service.runtime();
        let Some(run) = runtime
            .backtests
            .get_run(run_id)
            .map_err(|_| adapter_error("runner bridge backtest state could not be read"))?
        else {
            return Ok(None);
        };
        let stored_manifest = runtime
            .backtests
            .get_manifest(run_id)
            .map_err(|_| adapter_error("runner bridge backtest manifest could not be read"))?
            .ok_or_else(|| mismatch("runner bridge backtest manifest is missing"))?;
        if stored_manifest.verify().is_err()
            || stored_manifest != command.backtest_manifest
            || run.manifest_hash != command.backtest_manifest.manifest_hash
            || run.request_hash != command.backtest_manifest.content.request_hash
            || run.idempotency_key != command.backtest_manifest.content.idempotency_key
        {
            return Err(mismatch(
                "runner bridge durable run does not match the submitted manifest",
            ));
        }
        Ok(Some(run))
    }

    fn drive(
        &self,
        command: &RunnerBridgeCommand,
    ) -> Result<RunnerBridgeReceipt, RunnerBridgeError> {
        if let Some(receipt) = self.load_receipt(command)? {
            if receipt.state != RunnerBridgeReceiptState::Accepted {
                return Ok(receipt);
            }
        }
        let run_id = &command.backtest_manifest.content.run_id;
        if self.load_bound_run(command)?.is_none() {
            let admission = backtest_lifecycle::create_from_manifest(
                &self.service,
                command.backtest_manifest.clone(),
            );
            if admission.status >= 400 && self.load_bound_run(command)?.is_none() {
                return self.persist_receipt(
                    command,
                    failed_receipt(command, &admission.body, command.submitted_at_ms)?,
                );
            }
        }

        let state = self
            .load_bound_run(command)?
            .ok_or_else(|| adapter_error("runner bridge admitted run is unavailable"))?;
        if matches!(
            state.state,
            BacktestRunState::Queued | BacktestRunState::Running
        ) {
            let worker = format!("core-runner-{}", stable_suffix(&command.command_id));
            let _ = backtest_lifecycle::process(&self.service, &worker);
        }
        let run = self
            .load_bound_run(command)?
            .ok_or_else(|| adapter_error("runner bridge admitted run is unavailable"))?;
        let receipt = match run.state {
            BacktestRunState::Completed => {
                let result_hash = run.result_hash.clone().ok_or_else(|| {
                    adapter_error("completed backtest is missing result integrity")
                })?;
                terminal_receipt(
                    command,
                    RunnerBridgeReceiptState::Completed,
                    Some(format!("tradeassembly://backtests/{run_id}/report")),
                    Some(result_hash),
                    None,
                    run.updated_at_ms,
                )?
            }
            BacktestRunState::Failed | BacktestRunState::Canceled => terminal_receipt(
                command,
                RunnerBridgeReceiptState::Failed,
                None,
                None,
                Some(
                    run.failure_code
                        .unwrap_or_else(|| "backtest_execution_failed".to_string()),
                ),
                run.updated_at_ms,
            )?,
            _ => RunnerBridgeReceipt::accepted(
                command,
                &self.expected_core_release,
                receipt_id(command),
                recorded_at(command, command.submitted_at_ms),
            )?,
        };
        self.persist_receipt(command, receipt)
    }

    fn load_receipt(
        &self,
        command: &RunnerBridgeCommand,
    ) -> Result<Option<RunnerBridgeReceipt>, RunnerBridgeError> {
        let Some(value) = self
            .service
            .runtime()
            .storage
            .get_json(
                RECEIPTS_NS,
                &command_key(&command.workspace_id, &command.command_id),
            )
            .map_err(|_| adapter_error("runner bridge receipt could not be read"))?
        else {
            return Ok(None);
        };
        let receipt: RunnerBridgeReceipt = serde_json::from_value(value)
            .map_err(|_| adapter_error("runner bridge receipt record is corrupt"))?;
        receipt.verify(command, &self.expected_core_release)?;
        Ok(Some(receipt))
    }

    fn persist_receipt(
        &self,
        command: &RunnerBridgeCommand,
        candidate: RunnerBridgeReceipt,
    ) -> Result<RunnerBridgeReceipt, RunnerBridgeError> {
        candidate.verify(command, &self.expected_core_release)?;
        let storage = &self.service.runtime().storage;
        let key = command_key(&command.workspace_id, &command.command_id);
        let context = command_context(command)?;
        let candidate_value = serde_json::to_value(&candidate)
            .map_err(|_| adapter_error("runner bridge receipt could not be serialized"))?;
        match storage.put_json_if_absent(RECEIPTS_NS, &key, candidate_value.clone(), &context) {
            Ok(ImmutablePutOutcome::Created) => Ok(candidate),
            Ok(ImmutablePutOutcome::AlreadyPresent) | Err(_) => {
                let existing_value = storage
                    .get_json(RECEIPTS_NS, &key)
                    .map_err(|_| adapter_error("runner bridge receipt could not be read"))?
                    .ok_or_else(|| adapter_error("runner bridge receipt could not be stored"))?;
                let existing: RunnerBridgeReceipt = serde_json::from_value(existing_value.clone())
                    .map_err(|_| adapter_error("runner bridge receipt record is corrupt"))?;
                existing.verify(command, &self.expected_core_release)?;
                if existing.state != RunnerBridgeReceiptState::Accepted
                    || candidate.state == RunnerBridgeReceiptState::Accepted
                {
                    return Ok(existing);
                }
                match storage
                    .compare_and_put_json(
                        RECEIPTS_NS,
                        &key,
                        existing_value,
                        candidate_value,
                        &context,
                    )
                    .map_err(|_| adapter_error("runner bridge receipt could not be advanced"))?
                {
                    ComparePutOutcome::Updated => Ok(candidate),
                    ComparePutOutcome::Conflict => self
                        .load_receipt(command)?
                        .ok_or_else(|| adapter_error("runner bridge receipt disappeared")),
                }
            }
        }
    }
}

impl VersionedPort for DurableCoreRunnerBridge {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        vec![
            PortDescriptor::new(PortKind::CoreRunnerBridge, "durable-core-runner")
                .for_profiles(&["local", "self_hosted", "serverless"])
                .with_capabilities(&["hosted_deterministic_backtest"]),
        ]
    }
}

impl CoreRunnerBridgePort for DurableCoreRunnerBridge {
    fn dispatch(
        &self,
        command: &RunnerBridgeCommand,
    ) -> Result<RunnerBridgeReceipt, RunnerBridgeError> {
        self.bind_command(command)?;
        self.drive(command)
    }

    fn receipt(
        &self,
        workspace_id: &str,
        command_id: &str,
    ) -> Result<Option<RunnerBridgeReceipt>, RunnerBridgeError> {
        let Some(command) = self.load_command(workspace_id, command_id)? else {
            return Ok(None);
        };
        self.drive(&command).map(Some)
    }
}

fn terminal_receipt(
    command: &RunnerBridgeCommand,
    state: RunnerBridgeReceiptState,
    result_ref: Option<String>,
    result_sha256: Option<String>,
    failure_code: Option<String>,
    now_ms: i64,
) -> Result<RunnerBridgeReceipt, RunnerBridgeError> {
    let receipt = RunnerBridgeReceipt {
        schema: crate::ports::CORE_RUNNER_BRIDGE_SCHEMA.to_string(),
        bridge_version: crate::ports::CORE_RUNNER_BRIDGE_VERSION.to_string(),
        receipt_id: receipt_id(command),
        workspace_id: command.workspace_id.clone(),
        command_id: command.command_id.clone(),
        idempotency_key: command.idempotency_key.clone(),
        workload: command.workload,
        core_release: command.core_release.clone(),
        command_sha256: command.canonical_hash()?,
        state,
        result_ref,
        result_sha256,
        evidence_refs: vec![
            format!(
                "tradeassembly://backtests/{}/manifest.json",
                command.backtest_manifest.content.run_id
            ),
            format!(
                "tradeassembly://backtests/{}/journal",
                command.backtest_manifest.content.run_id
            ),
        ],
        failure_code,
        recorded_at_ms: recorded_at(command, now_ms),
    };
    receipt.verify(command, &command.core_release)?;
    Ok(receipt)
}

fn failed_receipt(
    command: &RunnerBridgeCommand,
    response: &Value,
    now_ms: i64,
) -> Result<RunnerBridgeReceipt, RunnerBridgeError> {
    let code = response
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .filter(|code| !code.trim().is_empty())
        .unwrap_or("backtest_admission_failed")
        .to_string();
    terminal_receipt(
        command,
        RunnerBridgeReceiptState::Failed,
        None,
        None,
        Some(code),
        now_ms,
    )
}

fn command_context(command: &RunnerBridgeCommand) -> Result<SideEffectContext, RunnerBridgeError> {
    Ok(SideEffectContext::new(
        AuthorityContext {
            actor: command.authority_ref.clone(),
            surface: "core-runner-bridge".to_string(),
            account_mode: "research".to_string(),
        },
        IdempotencyKey::new(format!(
            "core-runner:{}",
            stable_suffix(&command.canonical_hash()?)
        ))
        .map_err(|_| adapter_error("runner bridge persistence idempotency is invalid"))?,
    ))
}

fn command_key(workspace_id: &str, command_id: &str) -> String {
    stable_key("command", workspace_id, command_id)
}

fn idempotency_key(command: &RunnerBridgeCommand) -> String {
    stable_key(
        "idempotency",
        &command.workspace_id,
        command.idempotency_key.as_str(),
    )
}

fn stable_key(kind: &str, left: &str, right: &str) -> String {
    format!("{kind}-{:x}", Sha256::digest(format!("{left}\0{right}")))
}

fn stable_suffix(value: &str) -> String {
    format!("{:x}", Sha256::digest(value))[..16].to_string()
}

fn receipt_id(command: &RunnerBridgeCommand) -> String {
    format!(
        "runner-receipt-{}",
        stable_suffix(&format!(
            "{}:{}:{}",
            command.workspace_id, command.command_id, command.core_release
        ))
    )
}

fn recorded_at(command: &RunnerBridgeCommand, now_ms: i64) -> i64 {
    now_ms.max(command.submitted_at_ms)
}

fn validate_lookup(workspace_id: &str, command_id: &str) -> Result<(), RunnerBridgeError> {
    if workspace_id.trim().is_empty()
        || command_id.trim().is_empty()
        || workspace_id.len() > 256
        || command_id.len() > 256
    {
        return Err(RunnerBridgeError {
            code: RunnerBridgeErrorCode::MalformedCommand,
            message: "runner bridge receipt lookup is invalid".to_string(),
        });
    }
    Ok(())
}

fn conflict() -> RunnerBridgeError {
    RunnerBridgeError {
        code: RunnerBridgeErrorCode::IdempotencyConflict,
        message: "runner bridge command identity was reused".to_string(),
    }
}

fn adapter_error(message: &str) -> RunnerBridgeError {
    RunnerBridgeError {
        code: RunnerBridgeErrorCode::AdapterFailure,
        message: message.to_string(),
    }
}

fn mismatch(message: &str) -> RunnerBridgeError {
    RunnerBridgeError {
        code: RunnerBridgeErrorCode::ReceiptMismatch,
        message: message.to_string(),
    }
}
