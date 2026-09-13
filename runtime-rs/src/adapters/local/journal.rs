// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::journal::{JournalEvent, JournalPort};
use crate::ports::{
    EventAppend, EventStreamPort, FailureMode, PortDescriptor, PortKind, VersionedPort,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

const STREAM: &str = "local.journal";
const EVENT_TYPE: &str = "journal.event";
const PAGE_SIZE: usize = 256;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredJournalEvent {
    event_type: String,
    authority: crate::ports::AuthorityContext,
    idempotency_key: crate::ports::IdempotencyKey,
    payload: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<crate::ports::ObjectOwner>,
}

pub struct LocalJournal {
    events: Arc<dyn EventStreamPort>,
}

impl LocalJournal {
    pub fn new(events: Arc<dyn EventStreamPort>) -> Self {
        Self { events }
    }

    fn decode(record: crate::ports::EventRecord) -> Result<JournalEvent, String> {
        if record.stream != STREAM || record.event_type != EVENT_TYPE {
            return Err("journal record has an invalid event envelope".to_string());
        }
        if record.idempotency_key.as_str() != canonical_hash(&record.payload) {
            return Err("journal record integrity check failed".to_string());
        }
        let event = serde_json::from_value::<StoredJournalEvent>(record.payload)
            .map_err(|_| "journal record payload is invalid".to_string())?;
        if record.aggregate_id != event.authority.actor {
            return Err("journal record integrity check failed".to_string());
        }
        Ok(JournalEvent {
            event_type: event.event_type,
            authority: event.authority,
            idempotency_key: event.idempotency_key,
            payload: event.payload,
            owner: event.owner,
        })
    }
}

impl VersionedPort for LocalJournal {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::Evidence, STREAM)
            .for_profiles(&["local"])
            .with_capabilities(&["evidence.append", "evidence.replay"]);
        descriptor.failure_mode = FailureMode::LocalOnly;
        vec![descriptor]
    }
}

impl JournalPort for LocalJournal {
    fn record(&self, event: JournalEvent) -> Result<String, String> {
        let stored = StoredJournalEvent {
            event_type: event.event_type,
            authority: event.authority,
            idempotency_key: event.idempotency_key,
            payload: event.payload,
            owner: event.owner,
        };
        let payload = serde_json::to_value(&stored)
            .map_err(|_| "journal event could not be serialized".to_string())?;
        let key = canonical_hash(&payload);
        let append = EventAppend {
            stream: STREAM.to_string(),
            event_type: EVENT_TYPE.to_string(),
            aggregate_id: stored.authority.actor.clone(),
            payload,
            idempotency_key: crate::ports::IdempotencyKey::new(key)?,
            occurred_at_ms: 0,
            retention_until_ms: None,
        };
        let record = self.events.append(append)?;
        Ok(format!("journal-local-{:04}", record.sequence))
    }

    fn try_events(&self) -> Result<Vec<JournalEvent>, String> {
        let mut after = 0;
        let mut output = Vec::new();
        loop {
            let page = self.events.replay(STREAM, after, PAGE_SIZE)?;
            if page.is_empty() {
                break;
            }
            let mut previous = after;
            for record in page {
                if record.sequence <= previous {
                    return Err("journal sequence did not advance".to_string());
                }
                previous = record.sequence;
                output.push(Self::decode(record)?);
            }
            after = previous;
            if output.len() % PAGE_SIZE != 0 {
                break;
            }
        }
        Ok(output)
    }

    fn events(&self) -> Vec<JournalEvent> {
        self.try_events()
            .unwrap_or_else(|_| panic!("journal read failed"))
    }
}

fn canonical_hash(value: &Value) -> String {
    let mut digest = Sha256::new();
    digest.update(serde_json_canonicalizer::to_vec(value).expect("JSON values are serializable"));
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{AuthorityContext, IdempotencyKey};
    use serde_json::json;
    use std::thread;
    use tempfile::tempdir;

    fn event(actor: &str, payload: Value) -> JournalEvent {
        event_with_owner(actor, payload, None)
    }

    fn event_with_owner(
        actor: &str,
        payload: Value,
        owner: Option<crate::ports::ObjectOwner>,
    ) -> JournalEvent {
        JournalEvent {
            event_type: "draft.changed".to_string(),
            authority: AuthorityContext {
                actor: actor.to_string(),
                surface: "test".to_string(),
                account_mode: "paper".to_string(),
            },
            idempotency_key: IdempotencyKey::new("operation-reused").unwrap(),
            payload,
            owner,
        }
    }

    #[test]
    fn identical_event_with_different_owners_retains_both_across_restart() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let journal = LocalJournal::new(Arc::new(
            crate::adapters::local::operations::LocalSqliteOperations::new(
                path.display().to_string(),
                false,
            ),
        ));
        let owner_a = crate::ports::ObjectOwner {
            issuer: "issuer-a".to_string(),
            subject: "same-subject".to_string(),
            tenant_ref: "tenant".to_string(),
        };
        let owner_b = crate::ports::ObjectOwner {
            issuer: "issuer-b".to_string(),
            subject: "same-subject".to_string(),
            tenant_ref: "tenant".to_string(),
        };
        let first = event_with_owner("alice", json!({"v": 1}), Some(owner_a.clone()));
        let second = event_with_owner("alice", json!({"v": 1}), Some(owner_b.clone()));
        let first_id = journal.record(first).unwrap();
        let second_id = journal.record(second).unwrap();
        assert_ne!(first_id, second_id);
        assert_eq!(journal.try_events().unwrap().len(), 2);
        drop(journal);
        let reopened = LocalJournal::new(Arc::new(
            crate::adapters::local::operations::LocalSqliteOperations::new(
                path.display().to_string(),
                false,
            ),
        ));
        let events = reopened.try_events().unwrap();
        assert_eq!(events.len(), 2);
        assert!(events
            .iter()
            .any(|event| event.owner == Some(owner_a.clone())));
        assert!(events
            .iter()
            .any(|event| event.owner == Some(owner_b.clone())));
    }

    #[test]
    fn journal_retains_order_and_changed_full_events_across_restart() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let operations = Arc::new(
            crate::adapters::local::operations::LocalSqliteOperations::new(
                path.display().to_string(),
                false,
            ),
        );
        let journal = LocalJournal::new(operations.clone());
        assert_eq!(
            journal.record(event("alice", json!({"v": 1}))).unwrap(),
            "journal-local-0001"
        );
        assert_eq!(
            journal.record(event("alice", json!({"v": 2}))).unwrap(),
            "journal-local-0002"
        );
        assert_eq!(
            journal.record(event("bob", json!({"v": 2}))).unwrap(),
            "journal-local-0003"
        );
        assert_eq!(
            journal.record(event("bob", json!({"v": 2}))).unwrap(),
            "journal-local-0003"
        );
        drop(journal);
        let reopened = LocalJournal::new(Arc::new(
            crate::adapters::local::operations::LocalSqliteOperations::new(
                path.display().to_string(),
                false,
            ),
        ));
        let events = reopened.try_events().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].payload, json!({"v": 1}));
        assert_eq!(events[1].authority.actor, "alice");
        assert_eq!(events[2].authority.actor, "bob");
    }

    #[test]
    fn concurrent_identical_events_converge() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let operations = Arc::new(
            crate::adapters::local::operations::LocalSqliteOperations::new(
                path.display().to_string(),
                false,
            ),
        );
        // Initialize the schema before concurrent writers open connections.
        operations.replay(STREAM, 0, 1).unwrap();
        let journal = Arc::new(LocalJournal::new(operations));
        let handles = (0..8)
            .map(|_| {
                let journal = journal.clone();
                thread::spawn(move || journal.record(event("alice", json!({"v": 1}))))
            })
            .collect::<Vec<_>>();
        for handle in handles {
            let result = handle.join().unwrap();
            assert!(result.is_ok(), "journal append failed: {result:?}");
        }
        assert_eq!(journal.try_events().unwrap().len(), 1);
    }

    #[test]
    fn journal_reopen_is_verified_in_a_real_subprocess() {
        if let Ok(path) = std::env::var("JOURNAL_SUBPROCESS_PATH") {
            let journal = LocalJournal::new(Arc::new(
                crate::adapters::local::operations::LocalSqliteOperations::new(path, false),
            ));
            assert_eq!(
                journal.try_events().unwrap(),
                vec![event("alice", json!({"v": 1}))]
            );
            assert_eq!(
                journal.record(event("alice", json!({"v": 1}))).unwrap(),
                "journal-local-0001"
            );
            return;
        }
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let journal = LocalJournal::new(Arc::new(
            crate::adapters::local::operations::LocalSqliteOperations::new(
                path.display().to_string(),
                false,
            ),
        ));
        journal.record(event("alice", json!({"v": 1}))).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("adapters::local::journal::tests::journal_reopen_is_verified_in_a_real_subprocess")
            .arg("--exact")
            .env("JOURNAL_SUBPROCESS_PATH", path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed; 0 ignored"),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn corrupted_payload_and_authority_are_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let journal = LocalJournal::new(Arc::new(
            crate::adapters::local::operations::LocalSqliteOperations::new(
                path.display().to_string(),
                false,
            ),
        ));
        journal.record(event("alice", json!({"v": 1}))).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let (original_payload, original_key): (String, String) = connection
            .query_row(
                "SELECT payload_json, idempotency_key FROM runtime_events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        connection
            .execute(
                "UPDATE runtime_events SET payload_json=?1",
                [r#"{"event_type":"draft.changed","authority":{"actor":"alice","surface":"test","account_mode":"paper"},"idempotency_key":"operation-reused","payload":{"v":99}}"#],
            )
            .unwrap();
        assert!(journal.try_events().is_err());

        connection
            .execute(
                "UPDATE runtime_events SET payload_json=?1, idempotency_key=?2, aggregate_id=?3",
                [original_payload.as_str(), original_key.as_str(), "bob"],
            )
            .unwrap();
        assert!(journal.try_events().is_err());
    }

    #[test]
    fn sqlite_write_failure_cannot_return_a_journal_receipt() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.sqlite");
        let journal = LocalJournal::new(Arc::new(
            crate::adapters::local::operations::LocalSqliteOperations::new(
                path.display().to_string(),
                false,
            ),
        ));
        assert!(journal.try_events().unwrap().is_empty());
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_journal BEFORE INSERT ON runtime_events BEGIN SELECT RAISE(ABORT, 'private-fixture-sentinel'); END;").unwrap();
        let error = journal.record(event("alice", json!({"v": 1}))).unwrap_err();
        assert!(!error.contains("private-fixture-sentinel"));
        assert!(journal.try_events().unwrap().is_empty());
        connection
            .execute_batch("DROP TABLE runtime_events; CREATE TABLE runtime_events (invalid TEXT);")
            .unwrap();
        assert!(journal.try_events().is_err());
    }
}
