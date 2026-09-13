// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::domain::RobustnessRunState;
use crate::ports::{
    ComparePutOutcome, FailureMode, ImmutablePutOutcome, PortDescriptor, PortKind,
    RobustnessImmutableWrite, RobustnessRunRepository, SideEffectContext, StoragePort,
    VersionedPort,
};
use crate::robustness_contracts::{
    RobustnessAttempt, RobustnessLifecycleEvent, RobustnessResult, RobustnessRunManifest,
    RobustnessRunRecord, ROBUSTNESS_ATTEMPT_SCHEMA, ROBUSTNESS_RUN_SCHEMA,
};
use std::sync::Arc;

const MANIFESTS: &str = "robustness_manifests_v1";
const RESULTS: &str = "robustness_results_v1";
const RUNS: &str = "robustness_runs_v1";
const ATTEMPTS: &str = "robustness_attempts_v1";
const EVENTS: &str = "robustness_lifecycle_events_v1";

pub struct StorageRobustnessRunRepository {
    storage: Arc<dyn StoragePort>,
}
impl StorageRobustnessRunRepository {
    pub fn new(storage: Arc<dyn StoragePort>) -> Self {
        Self { storage }
    }
    fn immutable<T: serde::Serialize>(
        &self,
        namespace: &str,
        key: &str,
        value: &T,
        context: &SideEffectContext,
        conflict: &str,
    ) -> Result<RobustnessImmutableWrite, String> {
        match self.storage.put_json_if_absent(
            namespace,
            key,
            serde_json::to_value(value).map_err(|_| "robustness_storage_invalid".to_string())?,
            context,
        ) {
            Ok(ImmutablePutOutcome::Created) => Ok(RobustnessImmutableWrite::Created),
            Ok(ImmutablePutOutcome::AlreadyPresent) => Ok(RobustnessImmutableWrite::AlreadyPresent),
            Err(error) if error == "immutable_storage_conflict" => Err(conflict.to_string()),
            Err(error) => Err(error),
        }
    }
    fn verify_run(&self, run: &RobustnessRunRecord) -> Result<(), String> {
        if run.schema != ROBUSTNESS_RUN_SCHEMA
            || run.run_id.trim().is_empty()
            || run.manifest_hash.trim().is_empty()
        {
            return Err("robustness_run_integrity_failed".to_string());
        }
        let manifest = self
            .get_manifest(&run.run_id)?
            .ok_or_else(|| "robustness_manifest_integrity_failed".to_string())?;
        if manifest.manifest_hash != run.manifest_hash {
            return Err("robustness_manifest_integrity_failed".to_string());
        }
        match (&run.result, &run.result_hash) {
            (Some(result), Some(hash)) if run.state == RobustnessRunState::Completed => {
                result.verify()?;
                if hash != &result.result_hash
                    || result.content.run_id != run.run_id
                    || result.content.manifest_hash != run.manifest_hash
                    || result.content.source != manifest.content.source
                {
                    return Err("robustness_result_integrity_failed".to_string());
                }
                if self.get_result(&run.run_id)?.as_ref() != Some(result) {
                    return Err("robustness_result_integrity_failed".to_string());
                }
            }
            (None, None) if run.state != RobustnessRunState::Completed => {}
            _ => return Err("robustness_result_integrity_failed".to_string()),
        }
        let mut previous = None;
        for (index, event) in run.events.iter().enumerate() {
            if event.sequence != (index + 1) as i64
                || event.run_id != run.run_id
                || event.previous_event_hash != previous
            {
                return Err("robustness_lifecycle_event_integrity_failed".to_string());
            }
            event.verify_hash()?;
            previous = Some(event.event_hash.clone());
        }
        if run.sequence != run.events.len() as i64 {
            return Err("robustness_run_integrity_failed".to_string());
        }
        Ok(())
    }
}
impl VersionedPort for StorageRobustnessRunRepository {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(
            PortKind::Storage,
            format!("{}.robustness", self.storage.adapter_name()),
        )
        .for_profiles(&["local", "self_hosted"])
        .with_capabilities(&[
            "robustness.manifest.immutable",
            "robustness.result.immutable",
            "robustness.lifecycle.compare_and_put",
            "robustness.lifecycle.append_only",
        ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        vec![descriptor]
    }
}
impl RobustnessRunRepository for StorageRobustnessRunRepository {
    fn put_manifest(
        &self,
        manifest: &RobustnessRunManifest,
        context: &SideEffectContext,
    ) -> Result<RobustnessImmutableWrite, String> {
        manifest.verify()?;
        self.immutable(
            MANIFESTS,
            &manifest.content.run_id,
            manifest,
            context,
            "robustness_manifest_write_conflict",
        )
    }
    fn get_manifest(&self, run_id: &str) -> Result<Option<RobustnessRunManifest>, String> {
        self.storage
            .get_json(MANIFESTS, run_id)?
            .map(|value| {
                let value: RobustnessRunManifest = serde_json::from_value(value)
                    .map_err(|_| "robustness_manifest_integrity_failed".to_string())?;
                value.verify()?;
                Ok(value)
            })
            .transpose()
    }
    fn put_result(
        &self,
        result: &RobustnessResult,
        context: &SideEffectContext,
    ) -> Result<RobustnessImmutableWrite, String> {
        result.verify()?;
        self.immutable(
            RESULTS,
            &result.content.run_id,
            result,
            context,
            "robustness_result_write_conflict",
        )
    }
    fn get_result(&self, run_id: &str) -> Result<Option<RobustnessResult>, String> {
        self.storage
            .get_json(RESULTS, run_id)?
            .map(|value| {
                let value: RobustnessResult = serde_json::from_value(value)
                    .map_err(|_| "robustness_result_integrity_failed".to_string())?;
                value.verify()?;
                if value.content.run_id != run_id {
                    return Err("robustness_result_integrity_failed".to_string());
                }
                Ok(value)
            })
            .transpose()
    }
    fn create_run(
        &self,
        run: &RobustnessRunRecord,
        context: &SideEffectContext,
    ) -> Result<RobustnessImmutableWrite, String> {
        if run.schema != ROBUSTNESS_RUN_SCHEMA || run.sequence != 0 || !run.events.is_empty() {
            return Err("robustness_run_invalid".to_string());
        }
        self.immutable(
            RUNS,
            &run.run_id,
            run,
            context,
            "robustness_run_write_conflict",
        )
    }
    fn get_run(&self, run_id: &str) -> Result<Option<RobustnessRunRecord>, String> {
        self.storage
            .get_json(RUNS, run_id)?
            .map(|value| {
                let run: RobustnessRunRecord = serde_json::from_value(value)
                    .map_err(|_| "robustness_run_integrity_failed".to_string())?;
                self.verify_run(&run)?;
                Ok(run)
            })
            .transpose()
    }
    fn list_runs(&self) -> Result<Vec<RobustnessRunRecord>, String> {
        let mut runs = self
            .storage
            .list_json(RUNS)?
            .into_iter()
            .map(|(_, value)| {
                let run: RobustnessRunRecord = serde_json::from_value(value)
                    .map_err(|_| "robustness_run_integrity_failed".to_string())?;
                self.verify_run(&run)?;
                Ok(run)
            })
            .collect::<Result<Vec<_>, String>>()?;
        runs.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        Ok(runs)
    }
    fn compare_and_put_run(
        &self,
        expected: &RobustnessRunRecord,
        replacement: &RobustnessRunRecord,
        context: &SideEffectContext,
    ) -> Result<bool, String> {
        if expected.run_id != replacement.run_id
            || replacement.schema != ROBUSTNESS_RUN_SCHEMA
            || replacement.sequence != expected.sequence + 1
            || replacement.fencing_token < expected.fencing_token
            || replacement.events.len() != expected.events.len() + 1
            || replacement.events[..expected.events.len()] != expected.events
        {
            return Err("robustness_run_transition_invalid".to_string());
        }
        let event = replacement.events.last().expect("checked");
        if event.from_state != expected.state
            || event.to_state != replacement.state
            || event.attempt_id != replacement.current_attempt_id
            || event.previous_event_hash
                != expected.events.last().map(|item| item.event_hash.clone())
        {
            return Err("robustness_run_transition_invalid".to_string());
        }
        event.verify_hash()?;
        let expected =
            serde_json::to_value(expected).map_err(|_| "robustness_storage_invalid".to_string())?;
        let replacement = serde_json::to_value(replacement)
            .map_err(|_| "robustness_storage_invalid".to_string())?;
        Ok(matches!(
            self.storage.compare_and_put_json(
                RUNS,
                &event.run_id,
                expected,
                replacement,
                context
            )?,
            ComparePutOutcome::Updated
        ))
    }
    fn put_attempt(
        &self,
        attempt: &RobustnessAttempt,
        context: &SideEffectContext,
    ) -> Result<(), String> {
        if attempt.schema != ROBUSTNESS_ATTEMPT_SCHEMA
            || attempt.attempt_id.trim().is_empty()
            || attempt.run_id.trim().is_empty()
            || attempt.ordinal == 0
        {
            return Err("robustness_attempt_invalid".to_string());
        }
        self.storage.put_json(
            ATTEMPTS,
            &attempt.attempt_id,
            serde_json::to_value(attempt).map_err(|_| "robustness_storage_invalid".to_string())?,
            context,
        )
    }
    fn get_attempt(&self, attempt_id: &str) -> Result<Option<RobustnessAttempt>, String> {
        self.storage
            .get_json(ATTEMPTS, attempt_id)?
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(|_| "robustness_attempt_integrity_failed".to_string())
            })
            .transpose()
    }
    fn list_attempts(&self, run_id: &str) -> Result<Vec<RobustnessAttempt>, String> {
        let mut attempts = self
            .storage
            .list_json(ATTEMPTS)?
            .into_iter()
            .map(|(_, value)| {
                serde_json::from_value(value)
                    .map_err(|_| "robustness_attempt_integrity_failed".to_string())
            })
            .collect::<Result<Vec<RobustnessAttempt>, String>>()?
            .into_iter()
            .filter(|item| item.run_id == run_id)
            .collect::<Vec<_>>();
        attempts.sort_by_key(|item| item.ordinal);
        Ok(attempts)
    }
    fn compare_and_put_attempt(
        &self,
        expected: &RobustnessAttempt,
        replacement: &RobustnessAttempt,
        context: &SideEffectContext,
    ) -> Result<bool, String> {
        if expected.attempt_id != replacement.attempt_id
            || expected.run_id != replacement.run_id
            || replacement.schema != ROBUSTNESS_ATTEMPT_SCHEMA
            || replacement.fencing_token < expected.fencing_token
            || replacement.updated_at_ms < expected.updated_at_ms
        {
            return Err("robustness_attempt_transition_invalid".to_string());
        }
        let expected =
            serde_json::to_value(expected).map_err(|_| "robustness_storage_invalid".to_string())?;
        let replacement = serde_json::to_value(replacement)
            .map_err(|_| "robustness_storage_invalid".to_string())?;
        let attempt_id = replacement["attemptId"]
            .as_str()
            .ok_or_else(|| "robustness_storage_invalid".to_string())?
            .to_string();
        Ok(matches!(
            self.storage.compare_and_put_json(
                ATTEMPTS,
                &attempt_id,
                expected,
                replacement,
                context
            )?,
            ComparePutOutcome::Updated
        ))
    }
    fn append_event(
        &self,
        event: &RobustnessLifecycleEvent,
        context: &SideEffectContext,
    ) -> Result<RobustnessImmutableWrite, String> {
        event.verify_hash()?;
        self.immutable(
            EVENTS,
            &event.event_id,
            event,
            context,
            "robustness_lifecycle_event_conflict",
        )
    }
    fn list_events(&self, run_id: &str) -> Result<Vec<RobustnessLifecycleEvent>, String> {
        Ok(self
            .get_run(run_id)?
            .map(|run| run.events)
            .unwrap_or_default())
    }
    fn find_run_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<RobustnessRunRecord>, String> {
        Ok(self
            .list_runs()?
            .into_iter()
            .find(|run| run.idempotency_key == idempotency_key))
    }
}
