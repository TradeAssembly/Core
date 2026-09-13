// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{SideEffectContext, VersionedPort};
use serde_json::Value;

pub trait EventBusPort: VersionedPort {
    fn publish(
        &self,
        topic: &str,
        payload: Value,
        context: &SideEffectContext,
    ) -> Result<(), String>;
    fn published(&self) -> Vec<(String, Value)>;
}
