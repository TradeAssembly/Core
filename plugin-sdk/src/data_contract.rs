//! Opt-in data-operation metadata. This does not extend or reinterpret wire V1.
//! The host loads this document from a verified package, pins its digest, and
//! compiles bindings before invocation. It grants no execution authority.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataContractVersion {
    #[serde(rename = "tradeassembly.operation_data.v1")]
    V1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaIdentity {
    pub schema_ref: String,
    /// Lowercase SHA-256 of the schema document's canonical JSON bytes.
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataShape {
    Scalar,
    Series,
    Table,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeContract {
    /// None denotes event data without a fixed interval, never a wildcard.
    pub interval_ms: Option<u64>,
    pub completed_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataPort {
    pub schema: SchemaIdentity,
    pub shape: DataShape,
    pub time: TimeContract,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputPort {
    pub data: DataPort,
    pub required: bool,
    /// Capability dependency ID on the selected consumer operation. Required
    /// for cross-node bindings; prevents data edges bypassing graph resolution.
    pub dependency_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataApplicability {
    pub asset_classes: Vec<String>,
    pub instrument_families: Vec<String>,
    pub modes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessPolicy {
    /// No output until all required input windows are valid and complete.
    AllRequiredInputs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WarmupContract {
    /// Required observations per required input before the first valid output.
    pub minimum_observations: u64,
    /// Optional top-level positive integer parameter, e.g. a window size.
    pub observations_parameter: Option<String>,
    pub additional_observations: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatePartition {
    Instrument,
    Activation,
    ResearchRun,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationStateContract {
    pub schema: SchemaIdentity,
    pub version: String,
    pub partition: StatePartition,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationDataContract {
    pub schema_version: DataContractVersion,
    pub operation_id: String,
    /// Calculation/data semantics identity, separate from transport version.
    pub semantics_version: String,
    pub inputs: BTreeMap<String, InputPort>,
    pub outputs: BTreeMap<String, DataPort>,
    /// Embedded schema; only local fragment references are accepted by V1.
    pub parameter_schema: Value,
    pub applicability: DataApplicability,
    pub readiness: ReadinessPolicy,
    pub warmup: WarmupContract,
    pub state: Option<OperationStateContract>,
}

/// Host-owned batch metadata for the future evaluator. Event intervals,
/// availability time and arrival time are separate. Values must be validated
/// against the bound port schema. No state or cursor mutation is implied here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchProvenance {
    pub content_sha256: String,
    pub upstream_content_sha256: Vec<String>,
    pub event_start_ms: i64,
    pub event_end_ms: i64,
    pub known_at_ms: i64,
    pub arrived_at_ms: i64,
    pub completed: bool,
    pub source_revision: String,
}
