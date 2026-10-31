//! Pure execution calculations consume host-witnessed facts. Their output is a
//! proposal; it grants neither a capital reservation nor broker authority.
use crate::{
    execution::Decimal,
    execution_snapshot::{AccountSnapshot, InstrumentMetadata, Quote},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub const INPUT_SCHEMA: &str = "schema://tradeassembly/execution-calculation-input@1";
pub const OUTPUT_SCHEMA: &str = "schema://tradeassembly/execution-calculation-output@1";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalculationInput {
    pub schema_version: InputVersion,
    pub account: AccountSnapshot,
    pub metadata: InstrumentMetadata,
    pub quote: Quote,
    pub available_cash: Decimal,
    pub parameters: Value,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputVersion {
    #[serde(rename = "tradeassembly.execution_calculation_input.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputVersion {
    #[serde(rename = "tradeassembly.execution_calculation_output.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalculationOutput {
    pub schema_version: OutputVersion,
    pub quantity: Decimal,
    pub limit_price: Decimal,
    pub reason_codes: Vec<String>,
}
pub fn input_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["schemaVersion","account","metadata","quote","availableCash","parameters"],"properties":{
        "schemaVersion":{"const":"tradeassembly.execution_calculation_input.v1"},"account":crate::execution_snapshot::account_schema(),"metadata":crate::execution_snapshot::metadata_schema(),"quote":crate::execution_snapshot::quote_schema(),"availableCash":crate::execution_snapshot::decimal_schema(),"parameters":{"type":"object"}}})
}
pub fn output_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["schemaVersion","quantity","limitPrice","reasonCodes"],"properties":{"schemaVersion":{"const":"tradeassembly.execution_calculation_output.v1"},"quantity":crate::execution_snapshot::decimal_schema(),"limitPrice":crate::execution_snapshot::decimal_schema(),"reasonCodes":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":128}}}})
}
