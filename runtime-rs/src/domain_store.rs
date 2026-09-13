// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::domain::{IdempotencyRequirement, LifecycleFsm, StalePolicy, TransitionOutcome};
use crate::ports::{AuthorityContext, IdempotencyKey, SideEffectContext, StoragePort};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const RECORD_NS: &str = "domain_records";
const TRANSITION_NS: &str = "domain_transition_events";
const TRANSITION_IDEMPOTENCY_NS: &str = "domain_transition_idempotency";
const AGGREGATE_STATE_NS: &str = "domain_aggregate_state";
const RECORD_SCHEMA_VERSION: &str = "tradeassembly.domain_record.v1";
const TRANSITION_SCHEMA_VERSION: &str = "tradeassembly.domain_transition_event.v1";
const AGGREGATE_STATE_SCHEMA_VERSION: &str = "tradeassembly.domain_aggregate_state.v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredDomainRecord<T> {
    pub schema_version: String,
    pub record_type: String,
    pub record_id: String,
    pub record_hash: String,
    pub payload: T,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct StoredDomainRecordEnvelope {
    schema_version: String,
    record_type: String,
    record_id: String,
    record_hash: String,
    payload: Value,
}

pub struct DomainRecordStore<'a> {
    storage: &'a dyn StoragePort,
}

impl<'a> DomainRecordStore<'a> {
    pub fn new(storage: &'a dyn StoragePort) -> Self {
        Self { storage }
    }

    pub fn put_record<T: Serialize>(
        &self,
        record_type: &str,
        record_id: &str,
        payload: &T,
        context: &SideEffectContext,
    ) -> Result<StoredDomainRecord<Value>, String> {
        validate_key_part("record_type", record_type)?;
        validate_key_part("record_id", record_id)?;
        let payload = serde_json::to_value(payload).map_err(|error| error.to_string())?;
        let envelope = StoredDomainRecordEnvelope {
            schema_version: RECORD_SCHEMA_VERSION.to_string(),
            record_type: record_type.to_string(),
            record_id: record_id.to_string(),
            record_hash: hash_json(&payload)?,
            payload,
        };
        self.storage.put_json(
            RECORD_NS,
            &record_key(record_type, record_id),
            serde_json::to_value(&envelope).map_err(|error| error.to_string())?,
            context,
        )?;
        Ok(StoredDomainRecord {
            schema_version: envelope.schema_version,
            record_type: envelope.record_type,
            record_id: envelope.record_id,
            record_hash: envelope.record_hash,
            payload: envelope.payload,
        })
    }

    pub fn get_record<T: DeserializeOwned>(
        &self,
        record_type: &str,
        record_id: &str,
    ) -> Result<Option<StoredDomainRecord<T>>, String> {
        validate_key_part("record_type", record_type)?;
        validate_key_part("record_id", record_id)?;
        let Some(value) = self
            .storage
            .get_json(RECORD_NS, &record_key(record_type, record_id))?
        else {
            return Ok(None);
        };
        let envelope: StoredDomainRecordEnvelope =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        let actual_hash = hash_json(&envelope.payload)?;
        if actual_hash != envelope.record_hash {
            return Err("domain record hash mismatch".to_string());
        }
        Ok(Some(StoredDomainRecord {
            schema_version: envelope.schema_version,
            record_type: envelope.record_type,
            record_id: envelope.record_id,
            record_hash: envelope.record_hash,
            payload: serde_json::from_value(envelope.payload).map_err(|error| error.to_string())?,
        }))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainCommandEnvelope {
    pub command_id: String,
    pub source_interface: String,
    pub authority: AuthorityContext,
    pub idempotency_key: IdempotencyKey,
    pub expected_sequence: Option<u64>,
    pub evidence_refs: Vec<String>,
}

impl DomainCommandEnvelope {
    pub fn new(
        command_id: impl Into<String>,
        source_interface: impl Into<String>,
        authority: AuthorityContext,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self, String> {
        let command_id = command_id.into();
        let source_interface = source_interface.into();
        validate_key_part("command_id", &command_id)?;
        validate_key_part("source_interface", &source_interface)?;
        Ok(Self {
            command_id,
            source_interface,
            authority,
            idempotency_key,
            expected_sequence: None,
            evidence_refs: Vec::new(),
        })
    }

    pub fn with_expected_sequence(mut self, expected_sequence: u64) -> Self {
        self.expected_sequence = Some(expected_sequence);
        self
    }

    pub fn with_evidence_refs(mut self, evidence_refs: Vec<String>) -> Self {
        self.evidence_refs = evidence_refs;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersistedTransitionEvent {
    pub schema_version: String,
    pub event_id: String,
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub sequence: u64,
    pub event_type: String,
    pub command_id: String,
    pub command: String,
    pub source_interface: String,
    pub idempotency_key: String,
    pub authority: AuthorityContext,
    pub authority_class: String,
    pub idempotency_requirement: String,
    pub stale_policy: String,
    pub from_state: Value,
    pub to_state: Value,
    pub replay_inputs: Vec<String>,
    pub replay_outputs: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub payload: Value,
    pub previous_event_hash: Option<String>,
    pub event_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct IdempotencyRecord {
    schema_version: String,
    aggregate_type: String,
    aggregate_id: String,
    idempotency_key: String,
    event_key: String,
    event_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct AggregateStateRecord {
    schema_version: String,
    aggregate_type: String,
    aggregate_id: String,
    sequence: u64,
    state: Value,
    last_event_hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PersistedTransitionResult<S> {
    pub next_state: S,
    pub event: PersistedTransitionEvent,
    pub duplicate: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransitionApplyRequest<S, C> {
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub current_state: S,
    pub command: C,
    pub envelope: DomainCommandEnvelope,
    pub payload: Value,
}

impl<S, C> TransitionApplyRequest<S, C> {
    pub fn new(
        aggregate_type: impl Into<String>,
        aggregate_id: impl Into<String>,
        current_state: S,
        command: C,
        envelope: DomainCommandEnvelope,
        payload: Value,
    ) -> Self {
        Self {
            aggregate_type: aggregate_type.into(),
            aggregate_id: aggregate_id.into(),
            current_state,
            command,
            envelope,
            payload,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransitionReplayReport<S> {
    pub aggregate_type: String,
    pub aggregate_id: String,
    pub final_state: S,
    pub events: Vec<PersistedTransitionEvent>,
    pub integrity_ok: bool,
}

pub struct DomainTransitionStore<'a> {
    storage: &'a dyn StoragePort,
}

impl<'a> DomainTransitionStore<'a> {
    pub fn new(storage: &'a dyn StoragePort) -> Self {
        Self { storage }
    }

    pub fn apply_transition<F>(
        &self,
        request: TransitionApplyRequest<F::State, F::Command>,
        context: &SideEffectContext,
    ) -> Result<PersistedTransitionResult<F::State>, String>
    where
        F: LifecycleFsm,
        F::State: Serialize + DeserializeOwned,
    {
        let aggregate_type = request.aggregate_type.as_str();
        let aggregate_id = request.aggregate_id.as_str();
        let current_state = request.current_state;
        let command = request.command;
        let envelope = request.envelope;
        let payload = request.payload;
        validate_key_part("aggregate_type", aggregate_type)?;
        validate_key_part("aggregate_id", aggregate_id)?;
        let idem_key = idempotency_key(
            aggregate_type,
            aggregate_id,
            envelope.idempotency_key.as_str(),
        );
        if let Some(existing) = self
            .storage
            .get_json(TRANSITION_IDEMPOTENCY_NS, &idem_key)?
        {
            let existing: IdempotencyRecord =
                serde_json::from_value(existing).map_err(|error| error.to_string())?;
            let event = self.load_event_by_key(&existing.event_key)?;
            let next_state = serde_json::from_value(event.to_state.clone())
                .map_err(|error| error.to_string())?;
            return Ok(PersistedTransitionResult {
                next_state,
                event,
                duplicate: true,
            });
        }

        let outcome = F::transition(current_state, command)
            .map_err(|error| format!("{}: {}", error.reason, error.command))?;
        self.require_idempotency(&outcome, envelope.idempotency_key.as_str())?;

        let aggregate_key = aggregate_key(aggregate_type, aggregate_id);
        let prior_state = self.load_aggregate_state(&aggregate_key)?;
        let prior_sequence = prior_state
            .as_ref()
            .map(|state| state.sequence)
            .unwrap_or(0);
        self.check_stale_policy(&outcome, envelope.expected_sequence, prior_sequence)?;

        let sequence = prior_sequence + 1;
        let previous_event_hash = prior_state.and_then(|state| state.last_event_hash);
        let event_type = outcome
            .emitted_events
            .first()
            .copied()
            .ok_or_else(|| "transition must emit an event".to_string())?;
        let from_state = serde_json::to_value(current_state).map_err(|error| error.to_string())?;
        let to_state =
            serde_json::to_value(outcome.next_state).map_err(|error| error.to_string())?;
        let mut event = PersistedTransitionEvent {
            schema_version: TRANSITION_SCHEMA_VERSION.to_string(),
            event_id: format!("{}:{}:{:020}", aggregate_type, aggregate_id, sequence),
            aggregate_type: aggregate_type.to_string(),
            aggregate_id: aggregate_id.to_string(),
            sequence,
            event_type: event_type.to_string(),
            command_id: envelope.command_id,
            command: format!("{command:?}"),
            source_interface: envelope.source_interface,
            idempotency_key: envelope.idempotency_key.as_str().to_string(),
            authority: envelope.authority,
            authority_class: format!("{:?}", outcome.authority_class),
            idempotency_requirement: format!("{:?}", outcome.idempotency),
            stale_policy: format!("{:?}", outcome.stale_policy),
            from_state,
            to_state: to_state.clone(),
            replay_inputs: outcome
                .replay_inputs
                .into_iter()
                .map(str::to_string)
                .collect(),
            replay_outputs: outcome
                .replay_outputs
                .into_iter()
                .map(str::to_string)
                .collect(),
            evidence_refs: envelope.evidence_refs,
            payload,
            previous_event_hash,
            event_hash: String::new(),
        };
        event.event_hash = transition_event_hash(&event)?;
        let event_key = transition_event_key(aggregate_type, aggregate_id, sequence);

        self.storage.put_json(
            TRANSITION_NS,
            &event_key,
            serde_json::to_value(&event).map_err(|error| error.to_string())?,
            context,
        )?;
        self.storage.put_json(
            TRANSITION_IDEMPOTENCY_NS,
            &idem_key,
            serde_json::to_value(IdempotencyRecord {
                schema_version: TRANSITION_SCHEMA_VERSION.to_string(),
                aggregate_type: aggregate_type.to_string(),
                aggregate_id: aggregate_id.to_string(),
                idempotency_key: event.idempotency_key.clone(),
                event_key,
                event_hash: event.event_hash.clone(),
            })
            .map_err(|error| error.to_string())?,
            context,
        )?;
        self.storage.put_json(
            AGGREGATE_STATE_NS,
            &aggregate_key,
            serde_json::to_value(AggregateStateRecord {
                schema_version: AGGREGATE_STATE_SCHEMA_VERSION.to_string(),
                aggregate_type: aggregate_type.to_string(),
                aggregate_id: aggregate_id.to_string(),
                sequence,
                state: to_state,
                last_event_hash: Some(event.event_hash.clone()),
            })
            .map_err(|error| error.to_string())?,
            context,
        )?;

        Ok(PersistedTransitionResult {
            next_state: outcome.next_state,
            event,
            duplicate: false,
        })
    }

    pub fn list_transition_events(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
    ) -> Result<Vec<PersistedTransitionEvent>, String> {
        validate_key_part("aggregate_type", aggregate_type)?;
        validate_key_part("aggregate_id", aggregate_id)?;
        let prefix = format!("{}:{}:", aggregate_type, aggregate_id);
        let mut events = self
            .storage
            .list_json(TRANSITION_NS)?
            .into_iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(_, value)| serde_json::from_value(value).map_err(|error| error.to_string()))
            .collect::<Result<Vec<PersistedTransitionEvent>, String>>()?;
        events.sort_by_key(|event| event.sequence);
        Ok(events)
    }

    pub fn replay_state<S>(
        &self,
        aggregate_type: &str,
        aggregate_id: &str,
        initial_state: S,
    ) -> Result<TransitionReplayReport<S>, String>
    where
        S: Serialize + DeserializeOwned,
    {
        let events = self.list_transition_events(aggregate_type, aggregate_id)?;
        let mut expected_previous_hash = None;
        let mut final_state = initial_state;
        for event in &events {
            if event.previous_event_hash != expected_previous_hash {
                return Err("transition event hash chain mismatch".to_string());
            }
            let actual_hash = transition_event_hash(event)?;
            if actual_hash != event.event_hash {
                return Err("transition event hash mismatch".to_string());
            }
            final_state = serde_json::from_value(event.to_state.clone())
                .map_err(|error| error.to_string())?;
            expected_previous_hash = Some(event.event_hash.clone());
        }
        Ok(TransitionReplayReport {
            aggregate_type: aggregate_type.to_string(),
            aggregate_id: aggregate_id.to_string(),
            final_state,
            events,
            integrity_ok: true,
        })
    }

    fn load_aggregate_state(&self, key: &str) -> Result<Option<AggregateStateRecord>, String> {
        self.storage
            .get_json(AGGREGATE_STATE_NS, key)?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| error.to_string())
    }

    fn load_event_by_key(&self, event_key: &str) -> Result<PersistedTransitionEvent, String> {
        let value = self
            .storage
            .get_json(TRANSITION_NS, event_key)?
            .ok_or_else(|| "idempotency record points to missing transition event".to_string())?;
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    fn require_idempotency<S>(
        &self,
        outcome: &TransitionOutcome<S>,
        idempotency_key: &str,
    ) -> Result<(), String> {
        if !matches!(outcome.idempotency, IdempotencyRequirement::OptionalCache)
            && idempotency_key.trim().is_empty()
        {
            return Err("idempotency key is required for this transition".to_string());
        }
        Ok(())
    }

    fn check_stale_policy<S>(
        &self,
        outcome: &TransitionOutcome<S>,
        expected_sequence: Option<u64>,
        prior_sequence: u64,
    ) -> Result<(), String> {
        if matches!(
            outcome.stale_policy,
            StalePolicy::RejectExpectedVersion | StalePolicy::CompareAndSet
        ) {
            let Some(expected_sequence) = expected_sequence else {
                return Err("expected sequence is required for this transition".to_string());
            };
            if expected_sequence != prior_sequence {
                return Err(format!(
                    "stale transition: expected sequence {expected_sequence}, current sequence {prior_sequence}"
                ));
            }
        }
        Ok(())
    }
}

fn validate_key_part(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} is required"));
    }
    if value.contains("://") || value.contains('/') || value.contains('\\') {
        return Err(format!("{label} must be a stable storage key segment"));
    }
    Ok(())
}

fn record_key(record_type: &str, record_id: &str) -> String {
    format!("{record_type}:{record_id}")
}

fn aggregate_key(aggregate_type: &str, aggregate_id: &str) -> String {
    format!("{aggregate_type}:{aggregate_id}")
}

fn transition_event_key(aggregate_type: &str, aggregate_id: &str, sequence: u64) -> String {
    format!("{aggregate_type}:{aggregate_id}:{sequence:020}")
}

fn idempotency_key(aggregate_type: &str, aggregate_id: &str, idempotency_key: &str) -> String {
    format!("{aggregate_type}:{aggregate_id}:{idempotency_key}")
}

fn transition_event_hash(event: &PersistedTransitionEvent) -> Result<String, String> {
    hash_json(&json!({
        "schema_version": event.schema_version,
        "event_id": event.event_id,
        "aggregate_type": event.aggregate_type,
        "aggregate_id": event.aggregate_id,
        "sequence": event.sequence,
        "event_type": event.event_type,
        "command_id": event.command_id,
        "command": event.command,
        "source_interface": event.source_interface,
        "idempotency_key": event.idempotency_key,
        "authority": event.authority,
        "authority_class": event.authority_class,
        "idempotency_requirement": event.idempotency_requirement,
        "stale_policy": event.stale_policy,
        "from_state": event.from_state,
        "to_state": event.to_state,
        "replay_inputs": event.replay_inputs,
        "replay_outputs": event.replay_outputs,
        "evidence_refs": event.evidence_refs,
        "payload": event.payload,
        "previous_event_hash": event.previous_event_hash,
    }))
}

fn hash_json(value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let digest = Sha256::digest(bytes);
    Ok(format!("sha256:{digest:x}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters;
    use crate::domain::{
        BacktestRunCommand, BacktestRunFsm, BacktestRunState, LifecycleStatus, StrategyRecord,
    };

    fn context(idempotency_key: &str) -> SideEffectContext {
        SideEffectContext::new(
            AuthorityContext::local_cli(),
            IdempotencyKey::new(idempotency_key).expect("idempotency key"),
        )
    }

    fn command(idempotency_key: &str, expected_sequence: u64) -> DomainCommandEnvelope {
        DomainCommandEnvelope::new(
            format!("cmd-{idempotency_key}"),
            "unit-test",
            AuthorityContext::local_cli(),
            IdempotencyKey::new(idempotency_key).expect("idempotency key"),
        )
        .expect("command envelope")
        .with_expected_sequence(expected_sequence)
        .with_evidence_refs(vec!["evidence://fixture".to_string()])
    }

    #[test]
    fn canonical_records_are_stored_with_schema_and_payload_hash() {
        let runtime = adapters::local::test_runtime(format!(
            ".tradeassembly/domain-record-test-{}.db",
            std::process::id()
        ));
        let store = DomainRecordStore::new(runtime.storage.as_ref());
        let context = context("record-strategy-1");
        let record = StrategyRecord {
            strategy_id: "strat-domain-store".to_string(),
            status: LifecycleStatus::Draft,
            current_version_id: None,
            created_by_actor_id: "local-user".to_string(),
            workspace_id: "local".to_string(),
        };

        let stored = store
            .put_record("Strategy", &record.strategy_id, &record, &context)
            .expect("stored record");
        assert_eq!(stored.schema_version, RECORD_SCHEMA_VERSION);
        assert_eq!(stored.record_type, "Strategy");
        assert!(stored.record_hash.starts_with("sha256:"));

        let loaded: StoredDomainRecord<StrategyRecord> = store
            .get_record("Strategy", &record.strategy_id)
            .expect("loaded record")
            .expect("record exists");
        assert_eq!(loaded.payload, record);
        assert_eq!(loaded.record_hash, stored.record_hash);
    }

    #[test]
    fn fsm_transitions_are_persisted_deduped_and_replayed() {
        let runtime = adapters::local::test_runtime(format!(
            ".tradeassembly/domain-transition-test-{}.db",
            std::process::id()
        ));
        let store = DomainTransitionStore::new(runtime.storage.as_ref());
        let context = context("transition-context");

        let queued = store
            .apply_transition::<BacktestRunFsm>(
                TransitionApplyRequest::new(
                    "BacktestRun",
                    "backtest-1",
                    BacktestRunState::Ready,
                    BacktestRunCommand::Execute,
                    command("backtest-execute-1", 0),
                    json!({"manifest_hash": "sha256:manifest"}),
                ),
                &context,
            )
            .expect("queue transition");
        assert_eq!(queued.next_state, BacktestRunState::Queued);
        assert!(!queued.duplicate);
        assert_eq!(queued.event.sequence, 1);
        assert_eq!(queued.event.event_type, "backtest_run.queued");
        assert_eq!(queued.event.authority.actor, "local-user");

        let duplicate = store
            .apply_transition::<BacktestRunFsm>(
                TransitionApplyRequest::new(
                    "BacktestRun",
                    "backtest-1",
                    BacktestRunState::Ready,
                    BacktestRunCommand::Execute,
                    command("backtest-execute-1", 0),
                    json!({"manifest_hash": "sha256:manifest"}),
                ),
                &context,
            )
            .expect("duplicate transition");
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.event.event_hash, queued.event.event_hash);
        let advanced_state_retry = store
            .apply_transition::<BacktestRunFsm>(
                TransitionApplyRequest::new(
                    "BacktestRun",
                    "backtest-1",
                    BacktestRunState::Queued,
                    BacktestRunCommand::Execute,
                    command("backtest-execute-1", 1),
                    json!({"manifest_hash": "sha256:manifest"}),
                ),
                &context,
            )
            .expect("duplicate transition after state advance");
        assert!(advanced_state_retry.duplicate);
        assert_eq!(advanced_state_retry.next_state, BacktestRunState::Queued);
        assert_eq!(
            advanced_state_retry.event.event_hash,
            queued.event.event_hash
        );
        assert_eq!(
            store
                .list_transition_events("BacktestRun", "backtest-1")
                .expect("events")
                .len(),
            1
        );

        let stale = store
            .apply_transition::<BacktestRunFsm>(
                TransitionApplyRequest::new(
                    "BacktestRun",
                    "backtest-1",
                    BacktestRunState::Queued,
                    BacktestRunCommand::Start,
                    command("backtest-start-stale", 0),
                    json!({"attempt_id": "attempt-1"}),
                ),
                &context,
            )
            .expect_err("stale start must fail");
        assert!(stale.contains("stale transition"));

        let running = store
            .apply_transition::<BacktestRunFsm>(
                TransitionApplyRequest::new(
                    "BacktestRun",
                    "backtest-1",
                    BacktestRunState::Queued,
                    BacktestRunCommand::Start,
                    command("backtest-start-1", 1),
                    json!({"attempt_id": "attempt-1"}),
                ),
                &context,
            )
            .expect("start transition");
        assert_eq!(running.next_state, BacktestRunState::Running);
        assert_eq!(
            running.event.previous_event_hash,
            Some(queued.event.event_hash)
        );

        let replay = store
            .replay_state("BacktestRun", "backtest-1", BacktestRunState::Ready)
            .expect("replay");
        assert!(replay.integrity_ok);
        assert_eq!(replay.final_state, BacktestRunState::Running);
        assert_eq!(replay.events.len(), 2);
    }
}
