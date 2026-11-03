//! Explicit strategy-bound producer of candidate instruments. It does not
//! replace static/expression selectors or own positions, sizing or exits.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const SCREEN_REQUEST_SCHEMA: &str = "schema://tradeassembly/instrument-screen-request@1";
pub const SCREEN_RESULT_SCHEMA: &str = "schema://tradeassembly/instrument-screen-result@1";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum InstrumentScreenVersion {
    #[serde(rename = "tradeassembly.instrument_screen.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScreenCoveragePolicy {
    /// Complete only relative to the explicit configured provider input scopes.
    CompleteConfiguredInputs,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryDateBasis {
    SelectionUtcDate,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryDateBinding {
    pub basis: QueryDateBasis,
    pub offset_calendar_days: i32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenQuery {
    pub literal: BTreeMap<String, Value>,
    pub date_bindings: BTreeMap<String, QueryDateBinding>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollectionCoverageRequirement {
    /// RFC6901 pointer whose actual provider value must be boolean true.
    pub complete_pointer: String,
    /// Exact provider scope identity; the host does not infer a global market.
    pub scope_pointer: String,
    pub scope_value: Value,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenCollectionInput {
    pub capability_requirement_ref: String,
    pub response_schema_ref: String,
    pub query: ScreenQuery,
    pub max_age_ms: u64,
    pub coverage: CollectionCoverageRequirement,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstrumentScreen {
    pub version: InstrumentScreenVersion,
    pub capability_requirement_ref: String,
    pub semantics_version: String,
    pub inputs: BTreeMap<String, ScreenCollectionInput>,
    /// Exact candidate table and ID projection in one admitted named input.
    pub candidate_input: String,
    pub candidate_records_pointer: String,
    pub candidate_id_pointer: String,
    pub parameters: Value,
    pub coverage: ScreenCoveragePolicy,
    pub max_candidates: u32,
    pub top_n: u32,
    pub ttl_ms: u64,
}
fn name(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
fn pointer(v: &str) -> bool {
    if v.len() > 512 || !v.starts_with('/') {
        return false;
    }
    v.split('/').skip(1).all(|part| {
        let mut chars = part.chars();
        while let Some(c) = chars.next() {
            if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
                return false;
            }
        }
        true
    })
}
fn safe_value(value: &Value) -> bool {
    let mut stack = vec![(value, 0)];
    let mut nodes = 0;
    while let Some((v, depth)) = stack.pop() {
        nodes += 1;
        if nodes > 2048 || depth > 16 {
            return false;
        }
        match v {
            Value::Object(m) => {
                if m.len() > 128 || m.keys().any(|k| k.len() > 128) {
                    return false;
                }
                stack.extend(m.values().map(|v| (v, depth + 1)));
            }
            Value::Array(a) => {
                if a.len() > 256 {
                    return false;
                }
                stack.extend(a.iter().map(|v| (v, depth + 1)));
            }
            Value::String(s) if s.len() > 4096 => return false,
            Value::Number(n)
                if n.as_u64().is_some_and(|v| v > 9_007_199_254_740_991)
                    || n.as_i64()
                        .is_some_and(|v| v.unsigned_abs() > 9_007_199_254_740_991)
                    || n.as_f64()
                        .is_none_or(|v| !v.is_finite() || v.abs() > 9_007_199_254_740_991_f64) =>
            {
                return false
            }
            _ => {}
        }
    }
    true
}
impl InstrumentScreen {
    pub fn validate(&self) -> Result<(), &'static str> {
        if serde_json::to_vec(self)
            .map_err(|_| "instrument_screen_encoding_invalid")?
            .len()
            > 65536
            || self.inputs.is_empty()
            || self.inputs.len() > 8
            || !self.inputs.contains_key(&self.candidate_input)
            || (!self.candidate_records_pointer.is_empty()
                && !pointer(&self.candidate_records_pointer))
            || (!self.candidate_id_pointer.is_empty() && !pointer(&self.candidate_id_pointer))
            || self.top_n == 0
            || self.top_n > 256
            || self.max_candidates < self.top_n
            || self.max_candidates > 4096
            || self.ttl_ms == 0
            || self.ttl_ms > 9_007_199_254_740_991
            || !name(&self.capability_requirement_ref)
            || self.semantics_version.is_empty()
            || self.semantics_version.len() > 128
            || self.semantics_version.trim() != self.semantics_version
            || self.semantics_version.chars().any(char::is_control)
            || !self.parameters.is_object()
            || !safe_value(&self.parameters)
        {
            return Err("instrument_screen_bounds_invalid");
        }
        for (port, input) in &self.inputs {
            if !name(port)
                || !name(&input.capability_requirement_ref)
                || input.response_schema_ref.len() > 256
                || !input.response_schema_ref.starts_with("schema://")
                || input.max_age_ms == 0
                || input.max_age_ms > 9_007_199_254_740_991
                || !pointer(&input.coverage.complete_pointer)
                || !pointer(&input.coverage.scope_pointer)
                || input.coverage.complete_pointer == input.coverage.scope_pointer
                || input.coverage.scope_value.is_null()
                || !safe_value(&input.coverage.scope_value)
                || input.query.literal.len() + input.query.date_bindings.len() > 128
                || !safe_value(
                    &serde_json::to_value(&input.query.literal)
                        .map_err(|_| "instrument_screen_query_invalid")?,
                )
            {
                return Err("instrument_screen_input_invalid");
            }
            for (key, binding) in &input.query.date_bindings {
                if !name(key)
                    || input.query.literal.contains_key(key)
                    || binding.offset_calendar_days.unsigned_abs() > 3660
                {
                    return Err("instrument_screen_query_invalid");
                }
            }
            // Configuration and transport secrets are supplied by the host,
            // never through a published strategy's acquisition query.
            if input
                .query
                .literal
                .keys()
                .chain(input.query.date_bindings.keys())
                .any(|k| k == "config")
            {
                return Err("instrument_screen_query_invalid");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> Value {
        serde_json::json!({"version":"tradeassembly.instrument_screen.v1","capability_requirement_ref":"screen","semantics_version":"oi.rank.v1","inputs":{"contracts":{"capability_requirement_ref":"contracts","response_schema_ref":"schema://test/contracts@1","query":{"literal":{"instruments":["SPY"]},"date_bindings":{"asOfDate":{"basis":"selection_utc_date","offset_calendar_days":0},"expirationDateGte":{"basis":"selection_utc_date","offset_calendar_days":45},"expirationDateLte":{"basis":"selection_utc_date","offset_calendar_days":60}}},"max_age_ms":60000,"coverage":{"complete_pointer":"/coverage/complete","scope_pointer":"/coverage/scope","scope_value":"configured_underlyings"}}},"candidate_input":"contracts","candidate_records_pointer":"/observations","candidate_id_pointer":"/instrumentId","parameters":{"right":"call","rank":"open_interest","tie":"instrument_id"},"coverage":"complete_configured_inputs","max_candidates":4096,"top_n":5,"ttl_ms":60000})
    }
    #[test]
    fn screen_policy_is_closed_bounded_and_explicit() {
        let raw = policy();
        let p: InstrumentScreen = serde_json::from_value(raw.clone()).unwrap();
        p.validate().unwrap();
        for key in ["mode", "capital", "fallback"] {
            let mut invalid = raw.clone();
            invalid[key] = Value::Bool(true);
            assert!(serde_json::from_value::<InstrumentScreen>(invalid).is_err());
        }
        let mut invalid = raw;
        invalid["version"] = Value::String("tradeassembly.instrument_screen.v2".into());
        assert!(serde_json::from_value::<InstrumentScreen>(invalid).is_err());
    }
    #[test]
    fn ambiguous_queries_lossy_numbers_and_missing_coverage_reject() {
        let raw = policy();
        for (pointer, value) in [
            ("/top_n", serde_json::json!(0)),
            (
                "/parameters/window",
                serde_json::json!(9_007_199_254_740_992_u64),
            ),
            (
                "/inputs/contracts/query/literal/asOfDate",
                serde_json::json!("2026-10-31"),
            ),
            (
                "/inputs/contracts/coverage/complete_pointer",
                serde_json::json!("/bad~2pointer"),
            ),
            ("/inputs/contracts/coverage/scope_value", Value::Null),
        ] {
            let mut v = raw.clone();
            if let Some(slot) = v.pointer_mut(pointer) {
                *slot = value;
            } else {
                let (parent, key) = pointer.rsplit_once('/').unwrap();
                v.pointer_mut(parent).unwrap()[key] = value;
            }
            assert!(serde_json::from_value::<InstrumentScreen>(v)
                .unwrap()
                .validate()
                .is_err());
        }
    }
    #[test]
    fn old_selector_wire_stays_unchanged_and_screened_is_additive() {
        use crate::spec::{InstrumentSelector, SelectorType};
        let old = serde_json::json!({"selector_id":"spy","type":"normalized_instrument","normalized_id":"SPY"});
        for old in [
            old,
            serde_json::json!({"selector_id":"alias","type":"symbolic_alias","alias":"primary"}),
            serde_json::json!({"selector_id":"expression","type":"dynamic","expression":{"language":"cel@1","source":"[\"SPY\"]","result_type":"json","max_cost":10}}),
        ] {
            let decoded: InstrumentSelector = serde_json::from_value(old.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), old);
        }
        let new = serde_json::json!({"selector_id":"selected","type":"screened","screen":policy()});
        let decoded: InstrumentSelector = serde_json::from_value(new).unwrap();
        assert_eq!(decoded.selector_type, SelectorType::Screened);
        decoded.screen.unwrap().validate().unwrap();
    }
    fn bound_spec() -> Value {
        let mut spec = crate::spec::strategy_template_spec_payload("blank");
        let original = spec["signal_market_requirements"][0]["selectors"][0].clone();
        spec["signal_market_requirements"][0]["selectors"] = serde_json::json!([original,{"selector_id":"selected","type":"screened","screen":policy()}]);
        let base = spec["capability_requirements"]["required"][0].clone();
        let mut source = base.clone();
        source["requirement_id"] = serde_json::json!("contracts");
        source["capability"] = serde_json::json!("test.contracts.read@1");
        source["substep_refs"] = serde_json::json!([]);
        source["constraints"]["schema_refs"] = serde_json::json!(["schema://test/contracts@1"]);
        let mut screen = base;
        screen["requirement_id"] = serde_json::json!("screen");
        screen["capability"] = serde_json::json!("test.screen.read@1");
        screen["substep_refs"] = serde_json::json!([]);
        screen["dependency_refs"] = serde_json::json!(["contracts"]);
        screen["constraints"]["data_shapes"] = serde_json::json!(["instrument_set"]);
        screen["constraints"]["schema_refs"] =
            serde_json::json!([SCREEN_REQUEST_SCHEMA, SCREEN_RESULT_SCHEMA]);
        spec["capability_requirements"]["required"]
            .as_array_mut()
            .unwrap()
            .extend([source, screen]);
        spec
    }
    #[test]
    fn schema_binds_screen_and_inputs_to_mandatory_capability_graph() {
        let spec = bound_spec();
        let report = crate::spec::validate(&spec);
        assert!(report.valid, "{:?}", report.diagnostics);
        assert!(matches!(
            crate::strategy_kernel::CompiledStrategy::compile_json(&spec),
            Err(crate::strategy_kernel::KernelError::UnsupportedSemantics(
                "screened selector requires instrument-selection evaluator"
            ))
        ));
        let requirements = spec["capability_requirements"]["required"]
            .as_array()
            .unwrap();
        let screen_index = requirements
            .iter()
            .position(|r| r["requirement_id"] == "screen")
            .unwrap();
        let source_index = requirements
            .iter()
            .position(|r| r["requirement_id"] == "contracts")
            .unwrap();
        for field in ["dependency_refs", "constraints/schema_refs"] {
            let pointer = format!("/capability_requirements/required/{screen_index}/{field}");
            let mut invalid = spec.clone();
            *invalid.pointer_mut(&pointer).unwrap() = serde_json::json!([]);
            assert!(crate::spec::validate(&invalid)
                .diagnostics
                .iter()
                .any(|d| d.code == "SPEC_SCREEN_BINDING_INVALID"));
        }
        let mut invalid = spec.clone();
        invalid["stages"]["universe"]["enabled"] = Value::Bool(false);
        assert!(crate::spec::validate(&invalid)
            .diagnostics
            .iter()
            .any(|d| d.code == "SPEC_SCREEN_BINDING_INVALID"));
        let mut invalid = spec;
        invalid["capability_requirements"]["required"][source_index]["dependency_refs"] =
            serde_json::json!(["screen"]);
        assert!(crate::spec::validate(&invalid)
            .diagnostics
            .iter()
            .any(|d| d.code == "SPEC_CAPABILITY_CYCLE"));
    }
    #[test]
    fn canonical_integer_forms_validate_hash_and_load_identically() {
        let baseline = bound_spec();
        let mut integral = baseline.clone();
        integral["signal_market_requirements"][0]["selectors"][1]["screen"]["top_n"] =
            serde_json::json!(5.0);
        assert!(crate::spec::validate(&integral).valid);
        assert_eq!(
            crate::spec::canonical_hash(&baseline).unwrap(),
            crate::spec::canonical_hash(&integral).unwrap()
        );
        let parsed = crate::spec::StrategySpec::model_validate(integral).unwrap();
        assert_eq!(
            parsed.signal_market_requirements[0].selectors[1]
                .screen
                .as_ref()
                .unwrap()
                .top_n,
            5
        );
    }
}
