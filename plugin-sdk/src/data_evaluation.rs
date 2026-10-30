//! Bounded data evaluation payload carried inside the existing wire envelope.
//! Values, readiness and event metadata are separate; missing data is never false.
use crate::data_contract::{BatchProvenance, DataShape, SchemaIdentity};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const EVALUATION_RESPONSE_SCHEMA: &str = "schema://tradeassembly/data-evaluation-response@1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EvaluationVersion {
    #[serde(rename = "tradeassembly.data_evaluation.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvaluationWindow {
    pub event_start_ms: i64,
    pub event_end_ms: i64,
    pub as_of_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PointValue {
    Ready(Value),
    WarmingUp,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataPoint {
    pub instrument_id: String,
    pub source_event_id: String,
    pub provenance: BatchProvenance,
    pub result: PointValue,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataBatch {
    pub schema: SchemaIdentity,
    pub shape: DataShape,
    pub points: Vec<DataPoint>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataEvaluationRequest {
    pub schema_version: EvaluationVersion,
    pub plan_sha256: String,
    pub node_id: String,
    pub semantics_version: String,
    pub instruments: Vec<String>,
    pub window: EvaluationWindow,
    pub parameters: Value,
    pub inputs: BTreeMap<String, DataBatch>,
    pub prior_state: BTreeMap<String, Value>,
    pub max_points: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataEvaluationResponse {
    pub schema_version: EvaluationVersion,
    pub outputs: BTreeMap<String, DataBatch>,
    pub state: BTreeMap<String, Value>,
}

/// Semantic point identity excludes arrival time and the hash field itself.
/// Redelivery can have a later arrival without changing the source observation.
pub fn point_sha256(point: &DataPoint) -> Result<String, serde_json::Error> {
    crate::canonical::canonical_sha256(&serde_json::json!({
        "instrumentId":point.instrument_id,"sourceEventId":point.source_event_id,
        "eventStartMs":point.provenance.event_start_ms,"eventEndMs":point.provenance.event_end_ms,
        "knownAtMs":point.provenance.known_at_ms,"completed":point.provenance.completed,
        "sourceRevision":point.provenance.source_revision,
        "upstreamContentSha256":point.provenance.upstream_content_sha256,"result":point.result
    }))
}

/// Separates the host-owned configuration field from the closed evaluation
/// request. Existing wire V1 hosts add `config` to operation payloads. Unknown
/// evaluation fields still fail deserialization; configuration never grants
/// credentials or authority.
pub fn decode_operation_payload(
    mut payload: Value,
) -> Result<(DataEvaluationRequest, BTreeMap<String, Value>), String> {
    let object = payload
        .as_object_mut()
        .ok_or("data_evaluation_payload_invalid")?;
    let config = match object.remove("config") {
        None => BTreeMap::new(),
        Some(Value::Object(values)) => values.into_iter().collect(),
        Some(_) => return Err("data_evaluation_configuration_invalid".into()),
    };
    let request = serde_json::from_value(payload).map_err(|_| "data_evaluation_payload_invalid")?;
    Ok((request, config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn host_config_is_separate_and_evaluation_fields_remain_closed() {
        let request = DataEvaluationRequest {
            schema_version: EvaluationVersion::V1,
            plan_sha256: "plan".into(),
            node_id: "node".into(),
            semantics_version: "semantics.1".into(),
            instruments: vec!["instrument".into()],
            window: EvaluationWindow {
                event_start_ms: 1,
                event_end_ms: 2,
                as_of_ms: 2,
            },
            parameters: json!({}),
            inputs: BTreeMap::new(),
            prior_state: BTreeMap::new(),
            max_points: 1,
        };
        let mut payload = serde_json::to_value(&request).unwrap();
        payload["config"] = json!({"providerSetting":"explicit"});
        let (decoded, config) = decode_operation_payload(payload.clone()).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(config["providerSetting"], "explicit");
        payload["undeclared"] = json!(true);
        assert_eq!(
            decode_operation_payload(payload).unwrap_err(),
            "data_evaluation_payload_invalid"
        );
        let mut malformed = serde_json::to_value(&request).unwrap();
        malformed["config"] = Value::Null;
        assert_eq!(
            decode_operation_payload(malformed).unwrap_err(),
            "data_evaluation_configuration_invalid"
        );
    }
}
