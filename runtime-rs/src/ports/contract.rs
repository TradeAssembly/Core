// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PORT_CONTRACT_VERSION: &str = "tradeassembly.port.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortKind {
    Storage,
    DurableQueue,
    EventStream,
    Scheduler,
    Outbox,
    Inbox,
    Lease,
    Identity,
    Policy,
    Evidence,
    Credentials,
    Plugins,
    Telemetry,
    Clock,
    Exports,
    CoreRunnerBridge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureMode {
    FailClosed,
    DegradedReadOnly,
    LocalOnly,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortDescriptor {
    pub kind: PortKind,
    pub contract_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub supported_profiles: Vec<String>,
    pub capabilities: Vec<String>,
    pub health_check: String,
    pub configuration_schema: Value,
    pub secret_fields: Vec<String>,
    pub redaction_rules: Vec<String>,
    pub failure_mode: FailureMode,
    pub fixture_id: String,
    pub conformance_suite_id: String,
}

impl PortDescriptor {
    pub fn new(kind: PortKind, adapter_id: impl Into<String>) -> Self {
        Self {
            kind,
            contract_version: PORT_CONTRACT_VERSION.to_string(),
            adapter_id: adapter_id.into(),
            adapter_version: env!("CARGO_PKG_VERSION").to_string(),
            supported_profiles: Vec::new(),
            capabilities: Vec::new(),
            health_check: "adapter.health.v1".to_string(),
            configuration_schema: serde_json::json!({"type": "object", "additionalProperties": false}),
            secret_fields: Vec::new(),
            redaction_rules: vec!["secret_refs_only".to_string()],
            failure_mode: FailureMode::FailClosed,
            fixture_id: "tradeassembly.port.fixture.v1".to_string(),
            conformance_suite_id: "tradeassembly.port.conformance.v1".to_string(),
        }
    }

    pub fn for_profiles(mut self, profiles: &[&str]) -> Self {
        self.supported_profiles = profiles.iter().map(|value| (*value).to_string()).collect();
        self
    }

    pub fn with_capabilities(mut self, capabilities: &[&str]) -> Self {
        self.capabilities = capabilities
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        self
    }
}

pub trait VersionedPort: Send + Sync {
    fn descriptors(&self) -> Vec<PortDescriptor>;
}
