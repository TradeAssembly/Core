//! Closed, bounded first-readiness metadata. These expressions count complete
//! input observations; they do not calculate indicators or reconstruct state.
use crate::data_contract::{DataContractVersion, OperationDataContract, StatePartition};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_HISTORY_OBSERVATIONS: u64 = 1_048_576;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryError {
    Bounds,
    UnknownParameter,
    InvalidParameter,
    Overflow,
    Negative,
    Zero,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Observations {
    Literal {
        value: u64,
    },
    Parameter {
        name: String,
    },
    Add {
        terms: Vec<Observations>,
    },
    Subtract {
        left: Box<Observations>,
        right: Box<Observations>,
    },
    Multiply {
        terms: Vec<Observations>,
    },
    Maximum {
        terms: Vec<Observations>,
    },
    FloorDivide {
        value: Box<Observations>,
        divisor: u64,
    },
    FloorSqrt {
        value: Box<Observations>,
    },
}
impl Observations {
    pub fn resolve(&self, parameters: &Value) -> Result<u64, HistoryError> {
        let value = self.evaluate(parameters, 0, &mut 0)?;
        if value == 0 {
            Err(HistoryError::Zero)
        } else {
            Ok(value)
        }
    }
    fn evaluate(
        &self,
        parameters: &Value,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<u64, HistoryError> {
        *nodes += 1;
        if depth > 8 || *nodes > 64 {
            return Err(HistoryError::Bounds);
        }
        let mut child = |value: &Observations| value.evaluate(parameters, depth + 1, nodes);
        let value = match self {
            Self::Literal { value } => *value,
            Self::Parameter { name } => {
                if name.is_empty() || name.len() > 128 {
                    return Err(HistoryError::InvalidParameter);
                }
                let v = parameters.get(name).ok_or(HistoryError::UnknownParameter)?;
                match v.as_u64() {
                    Some(value) => value,
                    _ => v
                        .as_f64()
                        .filter(|n| {
                            n.is_finite()
                                && *n >= 0.0
                                && n.fract() == 0.0
                                && *n <= MAX_HISTORY_OBSERVATIONS as f64
                        })
                        .map(|n| n as u64)
                        .ok_or(HistoryError::InvalidParameter)?,
                }
            }
            Self::Add { terms } | Self::Multiply { terms } | Self::Maximum { terms } => {
                if terms.is_empty() || terms.len() > 32 {
                    return Err(HistoryError::Bounds);
                }
                let mut total = if matches!(self, Self::Multiply { .. }) {
                    1u64
                } else {
                    0
                };
                for term in terms {
                    let value = child(term)?;
                    total = match self {
                        Self::Add { .. } => {
                            total.checked_add(value).ok_or(HistoryError::Overflow)?
                        }
                        Self::Multiply { .. } => {
                            total.checked_mul(value).ok_or(HistoryError::Overflow)?
                        }
                        _ => total.max(value),
                    };
                    if total > MAX_HISTORY_OBSERVATIONS {
                        return Err(HistoryError::Bounds);
                    }
                }
                total
            }
            Self::Subtract { left, right } => child(left)?
                .checked_sub(child(right)?)
                .ok_or(HistoryError::Negative)?,
            Self::FloorDivide { value, divisor } => {
                if *divisor == 0 || *divisor > MAX_HISTORY_OBSERVATIONS {
                    return Err(HistoryError::InvalidParameter);
                }
                child(value)? / divisor
            }
            Self::FloorSqrt { value } => {
                let n = child(value)?;
                let (mut low, mut high) = (0u64, n);
                while low < high {
                    let mid = low + (high - low).div_ceil(2);
                    if mid <= n / mid {
                        low = mid;
                    } else {
                        high = mid - 1;
                    }
                }
                low
            }
        };
        if value > MAX_HISTORY_OBSERVATIONS {
            Err(HistoryError::Bounds)
        } else {
            Ok(value)
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Partitioning {
    InstrumentIndependent,
    AllDeclaredContext,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredInputUnavailable {
    InvokeCompleteFrame,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reconstruction {
    CheckpointOrExplicitOrigin,
    FiniteWindow,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryContract {
    /// Output -> required input -> observations of that selected input port.
    pub first_ready: BTreeMap<String, BTreeMap<String, Observations>>,
    pub partitioning: Partitioning,
    pub required_input_unavailable: RequiredInputUnavailable,
    /// A first-readiness count is never silently used as a recursive replay horizon.
    pub reconstruction: Reconstruction,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryVersion {
    #[serde(rename = "tradeassembly.operation_data.v3")]
    V3,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationDataV3 {
    pub schema_version: HistoryVersion,
    /// The closed V2 contract remains intact; V3 adds explicit stronger history
    /// and isolation semantics. Old readers reject this outer V3 record.
    pub operation: OperationDataContract,
    pub history: HistoryContract,
}
impl OperationDataV3 {
    pub fn resolve(
        &self,
        parameters: &Value,
    ) -> Result<BTreeMap<String, BTreeMap<String, u64>>, String> {
        if self.operation.schema_version != DataContractVersion::V2
            || self
                .history
                .first_ready
                .keys()
                .ne(self.operation.outputs.keys())
        {
            return Err("data_history_output_contract_invalid".into());
        }
        if self.history.partitioning == Partitioning::InstrumentIndependent
            && self
                .operation
                .state
                .as_ref()
                .is_some_and(|state| state.partition != StatePartition::Instrument)
        {
            return Err("data_history_partition_invalid".into());
        }
        let base_warmup = match &self.operation.warmup.observations_parameter {
            Some(name) => Observations::Parameter { name: name.clone() }
                .resolve(parameters)
                .map_err(|_| "data_history_base_parameter_invalid")?,
            None => 0,
        }
        .checked_add(self.operation.warmup.additional_observations)
        .ok_or("data_history_base_overflow")?
        .max(self.operation.warmup.minimum_observations);
        let required = self
            .operation
            .inputs
            .iter()
            .filter(|(_, p)| p.required)
            .map(|(k, _)| k)
            .collect::<Vec<_>>();
        let mut result = BTreeMap::new();
        for (output, inputs) in &self.history.first_ready {
            if inputs.len() != required.len()
                || required.iter().any(|key| !inputs.contains_key(*key))
            {
                return Err(format!("data_history_input_contract_invalid:{output}"));
            }
            let mut resolved = BTreeMap::new();
            for (input, expression) in inputs {
                let value = expression.resolve(parameters).map_err(|e| {
                    format!("data_history_expression_invalid:{output}:{input}:{e:?}")
                })?;
                if value < base_warmup {
                    return Err(format!("data_history_below_base_minimum:{output}:{input}"));
                }
                resolved.insert(input.clone(), value);
            }
            result.insert(output.clone(), resolved);
        }
        Ok(result)
    }
}

/// Same-interval producer/consumer composition, without assuming an indicator or
/// silently converting interval/calendar units.
pub fn compose_aligned_history(upstream: u64, consumer: u64) -> Result<u64, HistoryError> {
    if upstream == 0 || consumer == 0 {
        return Err(HistoryError::Zero);
    }
    let result = upstream
        .checked_add(consumer - 1)
        .ok_or(HistoryError::Overflow)?;
    if result > MAX_HISTORY_OBSERVATIONS {
        Err(HistoryError::Bounds)
    } else {
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn p(name: &str) -> Observations {
        Observations::Parameter { name: name.into() }
    }
    fn n(value: u64) -> Observations {
        Observations::Literal { value }
    }
    #[test]
    fn readiness_arithmetic_is_parameterized_exact_and_bounded() {
        let h = Observations::Subtract {
            left: Box::new(Observations::Add {
                terms: vec![
                    p("window"),
                    Observations::FloorSqrt {
                        value: Box::new(p("window")),
                    },
                ],
            }),
            right: Box::new(n(1)),
        };
        assert_eq!(h.resolve(&json!({"window":9})), Ok(11));
        assert_eq!(h.resolve(&json!({"window":9.0})), Ok(11));
        let max = Observations::Maximum {
            terms: vec![p("a"), p("b")],
        };
        assert_eq!(max.resolve(&json!({"a":5,"b":8})), Ok(8));
        let div = Observations::FloorDivide {
            value: Box::new(p("n")),
            divisor: 2,
        };
        assert_eq!(div.resolve(&json!({"n":7})), Ok(3));
        assert_eq!(
            p("absent").resolve(&json!({})),
            Err(HistoryError::UnknownParameter)
        );
        assert!(h.resolve(&json!({"window":-1})).is_err());
        assert!(n(MAX_HISTORY_OBSERVATIONS + 1).resolve(&json!({})).is_err());
        assert!(Observations::Multiply {
            terms: vec![n(MAX_HISTORY_OBSERVATIONS), n(2)]
        }
        .resolve(&json!({}))
        .is_err());
        assert!(Observations::Subtract {
            left: Box::new(n(1)),
            right: Box::new(n(2))
        }
        .resolve(&json!({}))
        .is_err());
        let mut deep = n(1);
        for _ in 0..10 {
            deep = Observations::FloorSqrt {
                value: Box::new(deep),
            };
        }
        assert!(deep.resolve(&json!({})).is_err());
    }
    #[test]
    fn zero_operands_are_valid_inside_positive_bounded_history() {
        let history = Observations::Add {
            terms: vec![p("period"), n(1)],
        };
        assert_eq!(history.resolve(&json!({"period":0})), Ok(1));
        assert_eq!(history.resolve(&json!({"period":0.0})), Ok(1));
        assert_eq!(history.resolve(&json!({"period":2})), Ok(3));
        assert_eq!(
            p("period").resolve(&json!({"period":0})),
            Err(HistoryError::Zero)
        );
        assert_eq!(n(0).resolve(&json!({})), Err(HistoryError::Zero));
        assert_eq!(
            Observations::Add {
                terms: vec![n(0), n(1)]
            }
            .resolve(&json!({})),
            Ok(1)
        );
        assert!(history.resolve(&json!({"period":-1})).is_err());
        assert!(history.resolve(&json!({"period":0.5})).is_err());
        assert!(history
            .resolve(&json!({"period":MAX_HISTORY_OBSERVATIONS}))
            .is_err());
        assert!(Observations::Add {
            terms: vec![
                Observations::Multiply {
                    terms: vec![n(0), n(MAX_HISTORY_OBSERVATIONS + 1)]
                },
                n(1)
            ]
        }
        .resolve(&json!({}))
        .is_err());
    }

    #[test]
    fn output_histories_remain_distinct_and_same_interval_chains_compose() {
        let port = json!({"schema":{"schemaRef":"schema://example/number","sha256":"a".repeat(64)},"shape":"series","time":{"intervalMs":60000,"completedOnly":true}});
        let operation:OperationDataContract=serde_json::from_value(json!({"schemaVersion":"tradeassembly.operation_data.v2","operationId":"example.transform","semanticsVersion":"example.1","inputs":{"source":{"data":port,"required":true,"dependencyId":"source"}},"outputs":{"early":port,"late":port},"parameterSchema":{"type":"object"},"applicability":{"assetClasses":["equity"],"instrumentFamilies":["equity"],"modes":["backtest"]},"readiness":"all_required_inputs","warmup":{"minimumObservations":1,"observationsParameter":null,"additionalObservations":0},"state":null})).unwrap();
        let late = Observations::Subtract {
            left: Box::new(Observations::Add {
                terms: vec![
                    Observations::Maximum {
                        terms: vec![p("a"), p("b")],
                    },
                    p("tail"),
                ],
            }),
            right: Box::new(n(1)),
        };
        let metadata = OperationDataV3 {
            schema_version: HistoryVersion::V3,
            operation,
            history: HistoryContract {
                first_ready: BTreeMap::from([
                    ("early".into(), BTreeMap::from([("source".into(), p("a"))])),
                    ("late".into(), BTreeMap::from([("source".into(), late)])),
                ]),
                partitioning: Partitioning::InstrumentIndependent,
                required_input_unavailable: RequiredInputUnavailable::InvokeCompleteFrame,
                reconstruction: Reconstruction::CheckpointOrExplicitOrigin,
            },
        };
        let legacy = serde_json::to_value(&metadata.operation).unwrap();
        let legacy_dispatch: VersionedOperationData =
            serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(&legacy_dispatch).unwrap(), legacy);
        assert!(legacy_dispatch.v3().is_none());
        let v3_json = serde_json::to_value(&metadata).unwrap();
        let dispatch: VersionedOperationData = serde_json::from_value(v3_json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&dispatch).unwrap(), v3_json);
        assert!(dispatch.into_legacy().is_err());
        let mut unknown = v3_json;
        unknown["schemaVersion"] = json!("tradeassembly.operation_data.v99");
        assert!(serde_json::from_value::<VersionedOperationData>(unknown).is_err());
        let resolved = metadata.resolve(&json!({"a":3,"b":5,"tail":2})).unwrap();
        assert_eq!(resolved["early"]["source"], 3);
        assert_eq!(resolved["late"]["source"], 6);
        assert!(serde_json::from_value::<OperationDataContract>(
            serde_json::to_value(&metadata).unwrap()
        )
        .is_err());
        let first = compose_aligned_history(15, 14).unwrap();
        assert_eq!(first, 28);
        let second = compose_aligned_history(first, 3).unwrap();
        assert_eq!(second, 30);
        assert_eq!(compose_aligned_history(second, 3), Ok(32));
        assert!(compose_aligned_history(MAX_HISTORY_OBSERVATIONS, 2).is_err());
    }
}

/// Version dispatch preserves the exact legacy JSON document instead of adding
/// optional history fields that old versions could silently accept.
#[derive(Clone, Debug, PartialEq)]
pub enum VersionedOperationData {
    Legacy(OperationDataContract),
    V3(OperationDataV3),
}
impl VersionedOperationData {
    pub fn v3(&self) -> Option<&OperationDataV3> {
        match self {
            Self::V3(v) => Some(v),
            _ => None,
        }
    }
    pub fn into_legacy(self) -> Result<OperationDataContract, String> {
        match self {
            Self::Legacy(v) => Ok(v),
            Self::V3(_) => Err("plugin_data_contract_version_requires_v3".into()),
        }
    }
}
impl From<OperationDataContract> for VersionedOperationData {
    fn from(value: OperationDataContract) -> Self {
        Self::Legacy(value)
    }
}
impl std::ops::Deref for VersionedOperationData {
    type Target = OperationDataContract;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Legacy(v) => v,
            Self::V3(v) => &v.operation,
        }
    }
}
impl std::ops::DerefMut for VersionedOperationData {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Legacy(v) => v,
            Self::V3(v) => &mut v.operation,
        }
    }
}
impl Serialize for VersionedOperationData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Legacy(v) => v.serialize(serializer),
            Self::V3(v) => v.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for VersionedOperationData {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        match value["schemaVersion"].as_str() {
            Some("tradeassembly.operation_data.v1" | "tradeassembly.operation_data.v2") => {
                serde_json::from_value(value)
                    .map(Self::Legacy)
                    .map_err(serde::de::Error::custom)
            }
            Some("tradeassembly.operation_data.v3") => {
                let record: OperationDataV3 =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                if record.operation.schema_version != DataContractVersion::V2 {
                    return Err(serde::de::Error::custom(
                        "V3 requires a closed V2 operation",
                    ));
                }
                Ok(Self::V3(record))
            }
            _ => Err(serde::de::Error::custom(
                "operation data version unsupported",
            )),
        }
    }
}
