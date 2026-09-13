// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::backtest_contracts::{
    canonical_hash, BacktestAttempt, BacktestLifecycleEvent, BacktestResult, BacktestRunManifest,
    BacktestRunRecord, BACKTEST_ATTEMPT_SCHEMA, BACKTEST_RUN_SCHEMA,
};
use crate::ports::{
    BacktestImmutableWrite, BacktestRunRepository, ComparePutOutcome, FailureMode,
    ImmutablePutOutcome, PortDescriptor, PortKind, SideEffectContext, StoragePort, VersionedPort,
};
use std::sync::Arc;

const MANIFESTS_NS: &str = "backtest_manifests_v1";
const RESULTS_NS: &str = "backtest_results_v1";
const RUNS_NS: &str = "backtest_runs_v2";
const ATTEMPTS_NS: &str = "backtest_attempts_v1";
const EVENTS_NS: &str = "backtest_lifecycle_events_v1";

pub struct StorageBacktestRunRepository {
    storage: Arc<dyn StoragePort>,
}

impl StorageBacktestRunRepository {
    pub fn new(storage: Arc<dyn StoragePort>) -> Self {
        Self { storage }
    }

    fn immutable_put<T: serde::Serialize>(
        &self,
        namespace: &str,
        key: &str,
        value: &T,
        context: &SideEffectContext,
        conflict_code: &str,
    ) -> Result<BacktestImmutableWrite, String> {
        let serialized =
            serde_json::to_value(value).map_err(|_| "backtest_storage_invalid".to_string())?;
        match self
            .storage
            .put_json_if_absent(namespace, key, serialized, context)
        {
            Ok(ImmutablePutOutcome::Created) => Ok(BacktestImmutableWrite::Created),
            Ok(ImmutablePutOutcome::AlreadyPresent) => Ok(BacktestImmutableWrite::AlreadyPresent),
            Err(error) if error == "immutable_storage_conflict" => Err(conflict_code.to_string()),
            Err(error) => Err(error),
        }
    }
}

impl VersionedPort for StorageBacktestRunRepository {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(
            PortKind::Storage,
            format!("{}.backtests", self.storage.adapter_name()),
        )
        .for_profiles(&["local", "self_hosted"])
        .with_capabilities(&[
            "backtest.manifest.immutable",
            "backtest.result.immutable",
            "backtest.lifecycle.compare_and_put",
            "backtest.lifecycle.append_only",
        ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        vec![descriptor]
    }
}

impl BacktestRunRepository for StorageBacktestRunRepository {
    fn put_manifest(
        &self,
        manifest: &BacktestRunManifest,
        context: &SideEffectContext,
    ) -> Result<BacktestImmutableWrite, String> {
        manifest.verify()?;
        self.immutable_put(
            MANIFESTS_NS,
            &manifest.content.run_id,
            manifest,
            context,
            "backtest_manifest_write_conflict",
        )
    }

    fn get_manifest(&self, run_id: &str) -> Result<Option<BacktestRunManifest>, String> {
        self.storage
            .get_json(MANIFESTS_NS, run_id)?
            .map(|value| -> Result<BacktestRunManifest, String> {
                let manifest: BacktestRunManifest = serde_json::from_value(value)
                    .map_err(|_| "backtest_manifest_integrity_failed".to_string())?;
                manifest.verify()?;
                Ok(manifest)
            })
            .transpose()
    }

    fn put_result(
        &self,
        result: &BacktestResult,
        context: &SideEffectContext,
    ) -> Result<BacktestImmutableWrite, String> {
        result.verify()?;
        self.immutable_put(
            RESULTS_NS,
            &result.content.run_id,
            result,
            context,
            "backtest_result_write_conflict",
        )
    }

    fn get_result(&self, run_id: &str) -> Result<Option<BacktestResult>, String> {
        let indexed = self
            .storage
            .get_json(RESULTS_NS, run_id)?
            .map(|value| -> Result<BacktestResult, String> {
                let result: BacktestResult = serde_json::from_value(value)
                    .map_err(|_| "backtest_result_integrity_failed".to_string())?;
                result.verify()?;
                if result.content.run_id != run_id {
                    return Err("backtest_result_integrity_failed".to_string());
                }
                Ok(result)
            })
            .transpose()?;
        if indexed.is_some() {
            return Ok(indexed);
        }
        let aggregate = self.get_run(run_id)?.and_then(|run| run.result);
        if let Some(result) = &aggregate {
            result.verify()?;
        }
        Ok(aggregate)
    }

    fn create_run(
        &self,
        run: &BacktestRunRecord,
        context: &SideEffectContext,
    ) -> Result<BacktestImmutableWrite, String> {
        if run.schema != BACKTEST_RUN_SCHEMA || run.run_id.trim().is_empty() {
            return Err("backtest_run_invalid".to_string());
        }
        self.immutable_put(
            RUNS_NS,
            &run.run_id,
            run,
            context,
            "backtest_run_write_conflict",
        )
    }

    fn get_run(&self, run_id: &str) -> Result<Option<BacktestRunRecord>, String> {
        self.storage
            .get_json(RUNS_NS, run_id)?
            .map(|value| -> Result<BacktestRunRecord, String> {
                let run: BacktestRunRecord = serde_json::from_value(value)
                    .map_err(|_| "backtest_run_integrity_failed".to_string())?;
                self.verify_run(&run)?;
                Ok(run)
            })
            .transpose()
    }

    fn list_runs(&self) -> Result<Vec<BacktestRunRecord>, String> {
        let mut runs = self
            .storage
            .list_json(RUNS_NS)?
            .into_iter()
            .map(|(_, value)| -> Result<BacktestRunRecord, String> {
                let run: BacktestRunRecord = serde_json::from_value(value)
                    .map_err(|_| "backtest_run_integrity_failed".to_string())?;
                self.verify_run(&run)?;
                Ok(run)
            })
            .collect::<Result<Vec<_>, _>>()?;
        runs.sort_by(|left: &BacktestRunRecord, right| left.run_id.cmp(&right.run_id));
        Ok(runs)
    }

    fn compare_and_put_run(
        &self,
        expected: &BacktestRunRecord,
        replacement: &BacktestRunRecord,
        context: &SideEffectContext,
    ) -> Result<bool, String> {
        if expected.run_id != replacement.run_id
            || replacement.sequence != expected.sequence + 1
            || replacement.schema != BACKTEST_RUN_SCHEMA
            || replacement.fencing_token < expected.fencing_token
            || !valid_aggregate_transition(expected, replacement)
        {
            return Err("backtest_run_transition_invalid".to_string());
        }
        let expected_value =
            serde_json::to_value(expected).map_err(|_| "backtest_storage_invalid".to_string())?;
        let replacement_value = serde_json::to_value(replacement)
            .map_err(|_| "backtest_storage_invalid".to_string())?;
        Ok(matches!(
            self.storage.compare_and_put_json(
                RUNS_NS,
                &replacement.run_id,
                expected_value,
                replacement_value,
                context,
            )?,
            ComparePutOutcome::Updated
        ))
    }

    fn put_attempt(
        &self,
        attempt: &BacktestAttempt,
        context: &SideEffectContext,
    ) -> Result<(), String> {
        if attempt.schema != BACKTEST_ATTEMPT_SCHEMA
            || attempt.attempt_id.trim().is_empty()
            || attempt.run_id.trim().is_empty()
            || attempt.progress_bps > 10_000
        {
            return Err("backtest_attempt_invalid".to_string());
        }
        self.storage.put_json(
            ATTEMPTS_NS,
            &attempt.attempt_id,
            serde_json::to_value(attempt).map_err(|_| "backtest_storage_invalid".to_string())?,
            context,
        )
    }

    fn compare_and_put_attempt(
        &self,
        expected: &BacktestAttempt,
        replacement: &BacktestAttempt,
        context: &SideEffectContext,
    ) -> Result<bool, String> {
        if expected.attempt_id != replacement.attempt_id
            || expected.run_id != replacement.run_id
            || replacement.schema != BACKTEST_ATTEMPT_SCHEMA
            || replacement.progress_bps > 10_000
            || replacement.fencing_token < expected.fencing_token
        {
            return Err("backtest_attempt_transition_invalid".to_string());
        }
        let expected_value =
            serde_json::to_value(expected).map_err(|_| "backtest_storage_invalid".to_string())?;
        let replacement_value = serde_json::to_value(replacement)
            .map_err(|_| "backtest_storage_invalid".to_string())?;
        Ok(matches!(
            self.storage.compare_and_put_json(
                ATTEMPTS_NS,
                &replacement.attempt_id,
                expected_value,
                replacement_value,
                context,
            )?,
            ComparePutOutcome::Updated
        ))
    }

    fn get_attempt(&self, attempt_id: &str) -> Result<Option<BacktestAttempt>, String> {
        self.storage
            .get_json(ATTEMPTS_NS, attempt_id)?
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(|_| "backtest_attempt_integrity_failed".to_string())
            })
            .transpose()
    }

    fn list_attempts(&self, run_id: &str) -> Result<Vec<BacktestAttempt>, String> {
        let mut attempts = self
            .storage
            .list_json(ATTEMPTS_NS)?
            .into_iter()
            .map(|(_, value)| {
                serde_json::from_value(value)
                    .map_err(|_| "backtest_attempt_integrity_failed".to_string())
            })
            .collect::<Result<Vec<BacktestAttempt>, _>>()?
            .into_iter()
            .filter(|attempt| attempt.run_id == run_id)
            .collect::<Vec<_>>();
        attempts.sort_by_key(|attempt| attempt.ordinal);
        Ok(attempts)
    }

    fn append_event(
        &self,
        event: &BacktestLifecycleEvent,
        context: &SideEffectContext,
    ) -> Result<BacktestImmutableWrite, String> {
        if event.run_id.trim().is_empty()
            || event.event_id.trim().is_empty()
            || event.sequence <= 0
            || event.event_hash.trim().is_empty()
        {
            return Err("backtest_lifecycle_event_invalid".to_string());
        }
        self.immutable_put(
            EVENTS_NS,
            &event.event_id,
            event,
            context,
            "backtest_lifecycle_event_conflict",
        )
    }

    fn list_events(&self, run_id: &str) -> Result<Vec<BacktestLifecycleEvent>, String> {
        Ok(self
            .get_run(run_id)?
            .map(|run| run.events)
            .unwrap_or_default())
    }

    fn find_run_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<BacktestRunRecord>, String> {
        Ok(self
            .list_runs()?
            .into_iter()
            .find(|run| run.idempotency_key == idempotency_key))
    }
}

impl StorageBacktestRunRepository {
    fn verify_run(&self, run: &BacktestRunRecord) -> Result<(), String> {
        if run.schema != BACKTEST_RUN_SCHEMA
            || run.run_id.trim().is_empty()
            || run.manifest_hash.trim().is_empty()
        {
            return Err("backtest_run_integrity_failed".to_string());
        }
        match (&run.result, &run.result_hash) {
            (Some(result), Some(result_hash))
                if run.state == crate::domain::BacktestRunState::Completed =>
            {
                result.verify()?;
                if result_hash != &result.result_hash
                    || result.content.run_id != run.run_id
                    || result.content.manifest_hash != run.manifest_hash
                {
                    return Err("backtest_result_integrity_failed".to_string());
                }
                let indexed: BacktestResult = self
                    .storage
                    .get_json(RESULTS_NS, &run.run_id)?
                    .ok_or_else(|| "backtest_result_integrity_failed".to_string())
                    .and_then(|value| {
                        serde_json::from_value(value)
                            .map_err(|_| "backtest_result_integrity_failed".to_string())
                    })?;
                indexed.verify()?;
                if indexed != *result {
                    return Err("backtest_result_integrity_failed".to_string());
                }
            }
            (None, None) if run.state != crate::domain::BacktestRunState::Completed => {}
            _ => return Err("backtest_result_integrity_failed".to_string()),
        }
        Ok(())
    }
}

fn valid_aggregate_transition(
    expected: &BacktestRunRecord,
    replacement: &BacktestRunRecord,
) -> bool {
    if replacement.events.len() != expected.events.len() + 1
        || replacement.events[..expected.events.len()] != expected.events
    {
        return false;
    }
    let event = replacement.events.last().expect("length checked");
    let previous = expected.events.last().map(|prior| prior.event_hash.clone());
    let expected_hash = canonical_hash(
        &serde_json::json!({
            "runId": event.run_id,
            "sequence": event.sequence,
            "from": event.from_state,
            "to": event.to_state,
            "attemptId": event.attempt_id,
            "previous": event.previous_event_hash,
            "detail": event.detail
        }),
        "backtest_event_hash_failed",
    );
    if event.run_id != replacement.run_id
        || event.sequence != replacement.sequence
        || event.from_state != expected.state
        || event.to_state != replacement.state
        || event.attempt_id != replacement.current_attempt_id
        || event.previous_event_hash != previous
        || expected_hash.as_deref() != Ok(event.event_hash.as_str())
    {
        return false;
    }
    match (&expected.result, &replacement.result) {
        (Some(prior), Some(next)) => {
            prior == next && expected.result_hash == replacement.result_hash
        }
        (Some(_), None) => false,
        (None, Some(result)) => {
            replacement.state == crate::domain::BacktestRunState::Completed
                && replacement.result_hash.as_deref() == Some(result.result_hash.as_str())
                && result.content.run_id == replacement.run_id
                && result.content.manifest_hash == replacement.manifest_hash
                && result.verify().is_ok()
        }
        (None, None) => replacement.result_hash.is_none(),
    }
}
