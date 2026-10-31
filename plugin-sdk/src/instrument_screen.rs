//! Stateless, named-document screening inside existing plugin wire V1. No
//! artificial bar intervals, credentials or execution authority are introduced.
use crate::{
    canonical::{canonical_sha256, normalize_numbers, MAX_EXACT_INTEGER},
    data_contract::SchemaIdentity,
    instrument_set::{digest, label, validate_members, SelectedInstrument, SelectionStatus},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
pub const SCREEN_INPUT_SCHEMA: &str = "schema://tradeassembly/instrument-screen-input@1";
pub const SCREEN_REQUEST_SCHEMA: &str = "schema://tradeassembly/instrument-screen-request@1";
pub const SCREEN_RESULT_SCHEMA: &str = "schema://tradeassembly/instrument-screen-result@1";
pub const MAX_SCREEN_BYTES: usize = 524_288;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenRequestVersion {
    #[serde(rename = "tradeassembly.instrument_screen_evaluation.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenResultVersion {
    #[serde(rename = "tradeassembly.instrument_screen_result.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenInput {
    pub schema: SchemaIdentity,
    pub collection_id: String,
    pub receipt_sha256: String,
    pub observed_at_ms: i64,
    pub known_at_ms: i64,
    pub payload: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenRequest {
    pub schema_version: ScreenRequestVersion,
    pub semantics_version: String,
    pub policy_sha256: String,
    pub universe_definition_sha256: String,
    pub knowledge_cutoff_ms: i64,
    pub candidate_input: String,
    pub candidate_records_pointer: String,
    pub candidate_id_pointer: String,
    pub max_candidates: u32,
    pub top_n: u32,
    pub parameters: Value,
    pub inputs: BTreeMap<String, ScreenInput>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenRejection {
    pub instrument_id: String,
    pub reason_codes: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenResult {
    pub schema_version: ScreenResultVersion,
    pub status: SelectionStatus,
    pub members: Vec<SelectedInstrument>,
    pub rejections: Vec<ScreenRejection>,
    pub issues: Vec<String>,
}
pub fn payload_budget(value: &Value) -> Result<(), String> {
    if serde_json::to_vec(value)
        .map_err(|_| "instrument_screen_encoding_invalid")?
        .len()
        > MAX_SCREEN_BYTES
    {
        return Err("instrument_screen_payload_too_large".into());
    }
    let mut stack = vec![(value, 0)];
    let mut nodes = 0;
    while let Some((item, depth)) = stack.pop() {
        nodes += 1;
        if nodes > 65_536 || depth > 24 {
            return Err("instrument_screen_payload_bounds_invalid".into());
        }
        match item {
            Value::Object(map) => {
                if map.len() > 256 || map.keys().any(|k| k.len() > 256) {
                    return Err("instrument_screen_payload_bounds_invalid".into());
                }
                stack.extend(map.values().map(|v| (v, depth + 1)));
            }
            Value::Array(rows) => {
                if rows.len() > 4096 {
                    return Err("instrument_screen_payload_bounds_invalid".into());
                }
                stack.extend(rows.iter().map(|v| (v, depth + 1)));
            }
            Value::String(s) if s.len() > 16_384 => {
                return Err("instrument_screen_payload_bounds_invalid".into())
            }
            _ => {}
        }
    }
    canonical_sha256(value).map_err(|_| "instrument_screen_numeric_range_invalid")?;
    Ok(())
}
pub fn decode_operation_payload(
    mut value: Value,
) -> Result<(ScreenRequest, BTreeMap<String, Value>), String> {
    payload_budget(&value)?;
    let object = value
        .as_object_mut()
        .ok_or("instrument_screen_request_invalid")?;
    let config = match object.remove("config") {
        None => BTreeMap::new(),
        Some(Value::Object(v)) => v.into_iter().collect(),
        Some(_) => return Err("instrument_screen_configuration_invalid".into()),
    };
    normalize_numbers(&mut value).map_err(|_| "instrument_screen_numeric_range_invalid")?;
    let request: ScreenRequest =
        serde_json::from_value(value).map_err(|_| "instrument_screen_request_invalid")?;
    request.validate()?;
    Ok((request, config))
}
impl ScreenRequest {
    pub fn candidate_ids(&self) -> Result<BTreeSet<String>, String> {
        self.validate()?;
        self.inputs[&self.candidate_input]
            .payload
            .pointer(&self.candidate_records_pointer)
            .and_then(Value::as_array)
            .ok_or("instrument_screen_candidates_invalid")?
            .iter()
            .map(|row| {
                row.pointer(&self.candidate_id_pointer)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or("instrument_screen_candidates_invalid".into())
            })
            .collect()
    }
    pub fn validate(&self) -> Result<(), String> {
        payload_budget(
            &serde_json::to_value(self).map_err(|_| "instrument_screen_request_invalid")?,
        )?;
        if !label(&self.semantics_version)
            || !digest(&self.policy_sha256)
            || !digest(&self.universe_definition_sha256)
            || self.knowledge_cutoff_ms <= 0
            || self.knowledge_cutoff_ms.unsigned_abs() > MAX_EXACT_INTEGER
            || self.max_candidates == 0
            || self.max_candidates > 4096
            || self.top_n == 0
            || self.top_n > 256
            || self.top_n > self.max_candidates
            || self.inputs.is_empty()
            || self.inputs.len() > 8
            || !self.inputs.contains_key(&self.candidate_input)
            || !self.parameters.is_object()
            || self.candidate_records_pointer.len() > 512
            || self.candidate_id_pointer.len() > 512
        {
            return Err("instrument_screen_request_invalid".into());
        }
        for (name, input) in &self.inputs {
            if !label(name)
                || name.len() > 128
                || !digest(&input.schema.sha256)
                || !digest(&input.receipt_sha256)
                || !input.schema.schema_ref.starts_with("schema://")
                || input.schema.schema_ref.len() > 256
                || !label(&input.collection_id)
                || input.observed_at_ms <= 0
                || input.known_at_ms < input.observed_at_ms
                || input.known_at_ms > self.knowledge_cutoff_ms
            {
                return Err("instrument_screen_input_invalid".into());
            }
        }
        let rows = self.inputs[&self.candidate_input]
            .payload
            .pointer(&self.candidate_records_pointer)
            .and_then(Value::as_array)
            .ok_or("instrument_screen_candidates_invalid")?;
        let mut ids = BTreeSet::new();
        if rows.len() > self.max_candidates as usize {
            return Err("instrument_screen_candidates_invalid".into());
        }
        for row in rows {
            let id = row
                .pointer(&self.candidate_id_pointer)
                .and_then(Value::as_str)
                .ok_or("instrument_screen_candidates_invalid")?;
            if !label(id) || !ids.insert(id) {
                return Err("instrument_screen_candidates_invalid".into());
            }
        }
        Ok(())
    }
}
pub fn decode_result(mut value: Value) -> Result<ScreenResult, String> {
    payload_budget(&value)?;
    normalize_numbers(&mut value).map_err(|_| "instrument_screen_numeric_range_invalid")?;
    let result: ScreenResult =
        serde_json::from_value(value).map_err(|_| "instrument_screen_result_invalid")?;
    result.validate()?;
    Ok(result)
}
impl ScreenResult {
    /// Complete results account for every candidate, including eligible members
    /// below top-N. Partial/failure never becomes a complete empty result.
    pub fn validate_against(&self, request: &ScreenRequest) -> Result<(), String> {
        self.validate()?;
        let expected = request.candidate_ids()?;
        let accounted = self
            .members
            .iter()
            .map(|m| m.instrument_id.clone())
            .chain(self.rejections.iter().map(|r| r.instrument_id.clone()))
            .collect::<BTreeSet<_>>();
        if self.members.len() > request.top_n as usize
            || !accounted.is_subset(&expected)
            || (matches!(
                self.status,
                SelectionStatus::CompleteNonempty | SelectionStatus::CompleteEmpty
            ) && accounted != expected)
        {
            return Err("instrument_screen_candidate_accounting_invalid".into());
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), String> {
        payload_budget(
            &serde_json::to_value(self).map_err(|_| "instrument_screen_result_invalid")?,
        )?;
        if self.members.len() > 256
            || self.rejections.len() > 4096
            || self.issues.len() > 64
            || self.issues.iter().any(|v| !label(v))
            || self.issues.iter().collect::<BTreeSet<_>>().len() != self.issues.len()
        {
            return Err("instrument_screen_result_invalid".into());
        }
        let mut ids = validate_members(&self.members)?;
        for rejection in &self.rejections {
            if !label(&rejection.instrument_id)
                || !ids.insert(rejection.instrument_id.as_str())
                || rejection.reason_codes.is_empty()
                || rejection.reason_codes.len() > 16
                || rejection.reason_codes.iter().any(|v| !label(v))
                || rejection.reason_codes.iter().collect::<BTreeSet<_>>().len()
                    != rejection.reason_codes.len()
            {
                return Err("instrument_screen_rejections_invalid".into());
            }
        }
        match self.status {
            SelectionStatus::CompleteNonempty
                if !self.members.is_empty() && self.issues.is_empty() => {}
            SelectionStatus::CompleteEmpty if self.members.is_empty() && self.issues.is_empty() => {
            }
            SelectionStatus::Partial if !self.issues.is_empty() => {}
            SelectionStatus::Failed if self.members.is_empty() && !self.issues.is_empty() => {}
            _ => return Err("instrument_screen_result_status_invalid".into()),
        }
        Ok(())
    }
}
fn string() -> Value {
    json!({"type":"string","minLength":1,"maxLength":256})
}
fn hash() -> Value {
    json!({"type":"string","pattern":"^[0-9a-f]{64}$"})
}
fn time() -> Value {
    json!({"type":"integer","minimum":1,"maximum":MAX_EXACT_INTEGER})
}
pub fn input_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["schema","collectionId","receiptSha256","observedAtMs","knownAtMs","payload"],"properties":{"schema":{"type":"object","additionalProperties":false,"required":["schemaRef","sha256"],"properties":{"schemaRef":string(),"sha256":hash()}},"collectionId":string(),"receiptSha256":hash(),"observedAtMs":time(),"knownAtMs":time(),"payload":true}})
}
pub fn input_identity() -> SchemaIdentity {
    SchemaIdentity {
        schema_ref: SCREEN_INPUT_SCHEMA.into(),
        sha256: canonical_sha256(&input_schema()).expect("fixed input schema"),
    }
}
pub fn request_schema() -> Value {
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,
        "required":["schemaVersion","semanticsVersion","policySha256","universeDefinitionSha256","knowledgeCutoffMs","candidateInput","candidateRecordsPointer","candidateIdPointer","maxCandidates","topN","parameters","inputs"],
        "properties":{"schemaVersion":{"const":"tradeassembly.instrument_screen_evaluation.v1"},"semanticsVersion":string(),"policySha256":hash(),"universeDefinitionSha256":hash(),"knowledgeCutoffMs":time(),"candidateInput":string(),"candidateRecordsPointer":{"type":"string","maxLength":512},"candidateIdPointer":{"type":"string","maxLength":512},"maxCandidates":{"type":"integer","minimum":1,"maximum":4096},"topN":{"type":"integer","minimum":1,"maximum":256},"parameters":{"type":"object"},"inputs":{"type":"object","minProperties":1,"maxProperties":8,"additionalProperties":input_schema()}}})
}
pub fn result_schema() -> Value {
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","additionalProperties":false,"required":["schemaVersion","status","members","rejections","issues"],"properties":{"schemaVersion":{"const":"tradeassembly.instrument_screen_result.v1"},"status":{"enum":["complete_nonempty","complete_empty","partial","failed"]},"members":{"type":"array","maxItems":256,"items":{"type":"object","additionalProperties":false,"required":["instrumentId","assetClass","instrumentFamily","rank","contextInstruments"],"properties":{"instrumentId":string(),"assetClass":string(),"instrumentFamily":string(),"rank":{"type":"integer","minimum":1,"maximum":256},"contextInstruments":{"type":"array","maxItems":32,"uniqueItems":true,"items":string()}}}},"rejections":{"type":"array","maxItems":4096,"items":{"type":"object","additionalProperties":false,"required":["instrumentId","reasonCodes"],"properties":{"instrumentId":string(),"reasonCodes":{"type":"array","minItems":1,"maxItems":16,"uniqueItems":true,"items":string()}}}},"issues":{"type":"array","maxItems":64,"uniqueItems":true,"items":string()}}})
}
pub fn request_identity() -> SchemaIdentity {
    SchemaIdentity {
        schema_ref: SCREEN_REQUEST_SCHEMA.into(),
        sha256: canonical_sha256(&request_schema()).expect("fixed request schema"),
    }
}
pub fn result_identity() -> SchemaIdentity {
    SchemaIdentity {
        schema_ref: SCREEN_RESULT_SCHEMA.into(),
        sha256: canonical_sha256(&result_schema()).expect("fixed result schema"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Value {
        json!({"schemaVersion":"tradeassembly.instrument_screen_evaluation.v1","semanticsVersion":"test.v1","policySha256":"a".repeat(64),"universeDefinitionSha256":"b".repeat(64),"knowledgeCutoffMs":20,"candidateInput":"contracts","candidateRecordsPointer":"/rows","candidateIdPointer":"/id","maxCandidates":10,"topN":1,"parameters":{},"inputs":{"contracts":{"schema":{"schemaRef":"schema://test/contracts@1","sha256":"c".repeat(64)},"collectionId":"collection_a","receiptSha256":"d".repeat(64),"observedAtMs":10,"knownAtMs":11,"payload":{"rows":[{"id":"A"},{"id":"B"}]}}}})
    }
    #[test]
    fn request_is_bounded_causal_and_has_exact_unique_candidates() {
        decode_operation_payload(request()).unwrap();
        for (pointer, value) in [
            ("/inputs/contracts/knownAtMs", json!(21)),
            (
                "/inputs/contracts/payload/rows",
                json!([{"id":"A"},{"id":"A"}]),
            ),
            ("/maxCandidates", json!(1)),
            ("/candidateIdPointer", json!("/missing")),
        ] {
            let mut v = request();
            *v.pointer_mut(pointer).unwrap() = value;
            assert!(decode_operation_payload(v).is_err());
        }
        let mut v = request();
        v["topN"] = json!(1.0);
        decode_operation_payload(v).unwrap();
        let mut v = request();
        v["authorization"] = json!("forbidden_field");
        assert!(decode_operation_payload(v).is_err());
    }
    #[test]
    fn result_keeps_failure_partial_and_no_match_distinct_and_accounts_unique_ids() {
        let complete = json!({"schemaVersion":"tradeassembly.instrument_screen_result.v1","status":"complete_empty","members":[],"rejections":[{"instrumentId":"A","reasonCodes":["missing_open_interest_date"]}],"issues":[]});
        decode_result(complete.clone()).unwrap();
        let mut partial = complete.clone();
        partial["status"] = json!("partial");
        assert!(decode_result(partial.clone()).is_err());
        partial["issues"] = json!(["incomplete_input"]);
        decode_result(partial).unwrap();
        let mut dup = complete;
        dup["rejections"]
            .as_array_mut()
            .unwrap()
            .push(json!({"instrumentId":"A","reasonCodes":["low_liquidity"]}));
        assert!(decode_result(dup).is_err());
        assert!(payload_budget(&json!("x".repeat(MAX_SCREEN_BYTES))).is_err());
    }
    #[test]
    fn complete_no_match_cannot_hide_uninspected_or_foreign_candidates() {
        let (req, _) = decode_operation_payload(request()).unwrap();
        let raw = json!({"schemaVersion":"tradeassembly.instrument_screen_result.v1","status":"complete_empty","members":[],"rejections":[{"instrumentId":"A","reasonCodes":["missing_oi"]}],"issues":[]});
        let mut result = decode_result(raw).unwrap();
        assert!(result.validate_against(&req).is_err());
        result.rejections.push(ScreenRejection {
            instrument_id: "B".into(),
            reason_codes: vec!["missing_oi".into()],
        });
        result.validate_against(&req).unwrap();
        result.rejections[1].instrument_id = "C".into();
        assert!(result.validate_against(&req).is_err());
    }
}
