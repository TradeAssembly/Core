// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    canonical_receipt_hash, core_receipt_schema, ClaimRunnerCommandRequest, ClockPort,
    CompleteRunnerCommandRequest, ConnectedRunnerError, ConnectedRunnerPoll, CorePaperRunnerPort,
    InstallationSignerPort, ProductRunnerTransportPort, RunnerCompletion, RunnerEntropyPort,
    RunnerOutcome, INSTALLATION_REQUEST_SCHEMA, RUNNER_LEASE_SECONDS,
};
use std::sync::Arc;

pub struct ConnectedPaperRunnerConsumer {
    expected_core_release: String,
    transport: Arc<dyn ProductRunnerTransportPort>,
    signer: Arc<dyn InstallationSignerPort>,
    runner: Arc<dyn CorePaperRunnerPort>,
    clock: Arc<dyn ClockPort>,
    entropy: Arc<dyn RunnerEntropyPort>,
}

impl ConnectedPaperRunnerConsumer {
    pub fn new(
        expected_core_release: impl Into<String>,
        transport: Arc<dyn ProductRunnerTransportPort>,
        signer: Arc<dyn InstallationSignerPort>,
        runner: Arc<dyn CorePaperRunnerPort>,
        clock: Arc<dyn ClockPort>,
        entropy: Arc<dyn RunnerEntropyPort>,
    ) -> Result<Self, ConnectedRunnerError> {
        let expected_core_release = expected_core_release.into();
        if expected_core_release.is_empty() || expected_core_release.len() > 256 {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        if signer.installation_id().is_empty() || signer.installation_id().len() > 256 {
            return Err(ConnectedRunnerError::invalid_contract());
        }
        Ok(Self {
            expected_core_release,
            transport,
            signer,
            runner,
            clock,
            entropy,
        })
    }

    pub fn poll_once(&self) -> Result<ConnectedRunnerPoll, ConnectedRunnerError> {
        let claim_time = self.clock.now_ms();
        let mut claim = ClaimRunnerCommandRequest {
            schema_version: INSTALLATION_REQUEST_SCHEMA.to_string(),
            installation_id: self.signer.installation_id().to_string(),
            request_id: self.entropy.opaque_id("claim-request")?,
            nonce: self.entropy.opaque_id("claim-nonce")?,
            issued_at_ms: claim_time,
            lease_seconds: RUNNER_LEASE_SECONDS,
            signature: String::new(),
        };
        claim.signature = self.signer.sign(&claim.signing_payload()?)?;
        let Some(lease) = self.transport.claim(&claim)? else {
            return Ok(ConnectedRunnerPoll::Idle);
        };
        lease.validate(
            self.signer.installation_id(),
            &self.expected_core_release,
            self.clock.now_ms(),
        )?;

        let receipt = self
            .runner
            .dispatch(&lease.command)
            .map_err(|_| ConnectedRunnerError::core_failure())?;
        receipt
            .verify(&lease.command, &self.expected_core_release)
            .map_err(|_| ConnectedRunnerError::core_failure())?;

        let completed_at_ms = self.clock.now_ms();
        if completed_at_ms >= lease.leased_until_ms {
            return Err(ConnectedRunnerError::stale_lease());
        }
        let completion = RunnerCompletion {
            outcome: RunnerOutcome::from(receipt.state),
            core_receipt_schema: core_receipt_schema(&receipt)?.to_string(),
            core_receipt_sha256: canonical_receipt_hash(&receipt)?,
            core_receipt_ref: format!(
                "tradeassembly://runner-receipts/{}/{}",
                lease.workspace_id, lease.command_id
            ),
            completed_at_ms,
        };
        let mut request = CompleteRunnerCommandRequest {
            schema_version: INSTALLATION_REQUEST_SCHEMA.to_string(),
            installation_id: self.signer.installation_id().to_string(),
            request_id: self.entropy.opaque_id("complete-request")?,
            nonce: self.entropy.opaque_id("complete-nonce")?,
            issued_at_ms: completed_at_ms,
            command_id: lease.command_id.clone(),
            lease_generation: lease.lease_generation,
            lease_token: lease.lease_token.clone(),
            command_sha256: lease.command_sha256.clone(),
            completion: completion.clone(),
            signature: String::new(),
        };
        request.signature = self.signer.sign(&request.signing_payload()?)?;
        let handoff = self.transport.complete(&request)?;
        handoff.verify(&lease, &completion)?;
        Ok(ConnectedRunnerPoll::Completed {
            receipt: Box::new(receipt),
            handoff: Box::new(handoff),
        })
    }
}
