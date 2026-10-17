// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{AuthorityContext, IdempotencyKey, ObjectOwner, VersionedPort};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct JournalEvent {
    pub event_type: String,
    pub authority: AuthorityContext,
    pub idempotency_key: IdempotencyKey,
    pub payload: Value,
    pub owner: Option<ObjectOwner>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalArchiveCursor {
    pub schema: String,
    pub stream: String,
    pub sequence: i64,
    pub record_hash: String,
    pub scope: String,
}

pub trait JournalPort: VersionedPort {
    fn record(&self, event: JournalEvent) -> Result<String, String>;
    fn try_events(&self) -> Result<Vec<JournalEvent>, String>;
    fn events(&self) -> Vec<JournalEvent>;
    /// Actual durable sequence for this owner's latest verified journal event.
    /// Never manufacture a count-based cursor for adapters without sequences.
    fn try_owner_cursor(&self, _owner: &ObjectOwner) -> Result<Option<LocalArchiveCursor>, String> {
        Err("journal_cursor_unsupported".to_string())
    }
}
