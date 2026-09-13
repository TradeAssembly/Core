// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{AuthorityContext, IdempotencyKey, VersionedPort};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct JournalEvent {
    pub event_type: String,
    pub authority: AuthorityContext,
    pub idempotency_key: IdempotencyKey,
    pub payload: Value,
}

pub trait JournalPort: VersionedPort {
    fn record(&self, event: JournalEvent) -> Result<String, String>;
    fn events(&self) -> Vec<JournalEvent>;
}
