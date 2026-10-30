//! Generic signal rules over verified pipeline outputs; no indicator calculations
//! or brokerage authority. A context instrument never becomes a traded instrument.
use crate::data_pipeline::{CompiledDataPlan, InstrumentContext, PipelineNode, PortBinding};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tradeassembly_plugin_sdk::data_evaluation::{DataEvaluationResponse, DataPoint, PointValue};

pub const CONFIG_KEY: &str = "tradeassembly.data_strategy";
pub type Frame = BTreeMap<String, DataEvaluationResponse>;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataStrategy {
    pub version: u32,
    pub nodes: Vec<PipelineNode>,
    pub bindings: Vec<PortBinding>,
    pub trade_instruments: Vec<InstrumentContext>,
    pub entry: Rule,
    pub exit: Rule,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operand {
    Output {
        node: String,
        port: String,
        instrument: String,
        sample: Sample,
    },
    Number {
        value: f64,
    },
    Boolean {
        value: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sample {
    Current,
    Previous,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
    Equal,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Rule {
    Compare {
        left: Operand,
        op: Comparison,
        right: Operand,
    },
    IsTrue {
        value: Operand,
    },
    All {
        rules: Vec<Rule>,
    },
    Any {
        rules: Vec<Rule>,
    },
    Not {
        rule: Box<Rule>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SignalValue {
    Ready { value: bool },
    WarmingUp,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignalFrame {
    pub event_end_ms: i64,
    pub known_at_ms: i64,
    pub arrived_at_ms: i64,
    pub trade_instruments: Vec<InstrumentContext>,
    pub entry: SignalValue,
    pub exit: SignalValue,
    pub upstream_content_sha256: Vec<String>,
}
pub fn discovery() -> Value {
    serde_json::json!({"id":"typed-data-signals-v1","configPointer":"/stages/evaluate/substeps/0/config/tradeassembly.data_strategy",
        "configSchema":schemars::schema_for!(DataStrategy),"signalEvaluationSupported":true,
        "backtestSupported":false,"statelessExecutionSupported":false,"executionAuthorityGranted":false,
        "prepareTool":"tradeassembly.strategy.data_plan.prepare","stepTool":"tradeassembly.strategy.data_run.step",
        "notice":"Signal evidence only. Backtest accounting and activation/tick consumption are not yet integrated. Data context is distinct from trading eligibility."})
}
impl DataStrategy {
    pub fn validate(&self, plan: &CompiledDataPlan) -> Result<(), String> {
        let mut expected = plan.request.clone();
        expected.nodes = self.nodes.clone();
        expected.bindings = self.bindings.clone();
        expected.trade_instruments = self.trade_instruments.clone();
        if !crate::data_pipeline::normalize_request(&mut expected).is_empty() {
            return Err("data_strategy_parameters_invalid".into());
        }
        if self.version != 1
            || expected != plan.request
            || self.trade_instruments.is_empty()
            || self.trade_instruments.len() > 64
            || self
                .trade_instruments
                .iter()
                .map(|i| &i.instrument_id)
                .collect::<BTreeSet<_>>()
                .len()
                != self.trade_instruments.len()
        {
            return Err("data_strategy_plan_mismatch".into());
        }
        let mut count = 0;
        validate_rule(&self.entry, plan, 0, &mut count)?;
        validate_rule(&self.exit, plan, 0, &mut count)
    }
    pub fn evaluate(
        &self,
        plan: &CompiledDataPlan,
        current: &Frame,
        previous: &Frame,
        time: i64,
        as_of: i64,
    ) -> Result<SignalFrame, String> {
        self.validate(plan)?;
        let mut evidence = Evidence {
            known: time,
            arrived: time,
            hashes: BTreeSet::new(),
        };
        let entry = eval_rule(&self.entry, plan, current, previous, time, &mut evidence)?;
        let exit = eval_rule(&self.exit, plan, current, previous, time, &mut evidence)?;
        if evidence.known > as_of {
            return Err("data_strategy_not_yet_known".into());
        }
        Ok(SignalFrame {
            event_end_ms: time,
            known_at_ms: evidence.known,
            arrived_at_ms: evidence.arrived,
            trade_instruments: plan.request.trade_instruments.clone(),
            entry: signal(entry),
            exit: signal(exit),
            upstream_content_sha256: evidence.hashes.into_iter().collect(),
        })
    }
}
fn signal(value: Option<bool>) -> SignalValue {
    value.map_or(SignalValue::WarmingUp, |value| SignalValue::Ready { value })
}
fn validate_rule(
    rule: &Rule,
    plan: &CompiledDataPlan,
    depth: usize,
    count: &mut usize,
) -> Result<(), String> {
    *count += 1;
    if depth > 8 || *count > 64 {
        return Err("data_strategy_rule_limit".into());
    }
    let operand = |operand: &Operand| -> Result<(), String> {
        match operand {
            Operand::Number { value }
                if !value.is_finite() || value.abs() > 9_007_199_254_740_991.0 =>
            {
                Err("data_strategy_number_invalid".into())
            }
            Operand::Output {
                node,
                port,
                instrument,
                ..
            } => {
                let definition = plan
                    .request
                    .nodes
                    .iter()
                    .find(|n| &n.graph_node_id == node)
                    .ok_or("data_strategy_node_missing")?;
                let contract = plan
                    .contracts
                    .iter()
                    .find(|c| &c.graph_node_id == node)
                    .ok_or("data_strategy_node_missing")?;
                if !contract.contract.outputs.contains_key(port)
                    || !definition
                        .context
                        .instruments
                        .iter()
                        .any(|i| &i.instrument_id == instrument)
                    || definition.context.interval_ms.is_none()
                {
                    return Err("data_strategy_output_context_invalid".into());
                }
                Ok(())
            }
            _ => Ok(()),
        }
    };
    match rule {
        Rule::Compare { left, right, .. } => {
            operand(left)?;
            operand(right)
        }
        Rule::IsTrue { value } => operand(value),
        Rule::All { rules } | Rule::Any { rules } => {
            if rules.is_empty() {
                return Err("data_strategy_empty_rule".into());
            }
            for rule in rules {
                validate_rule(rule, plan, depth + 1, count)?;
            }
            Ok(())
        }
        Rule::Not { rule } => validate_rule(rule, plan, depth + 1, count),
    }
}
struct Evidence {
    known: i64,
    arrived: i64,
    hashes: BTreeSet<String>,
}
fn operand_value(
    operand: &Operand,
    plan: &CompiledDataPlan,
    current: &Frame,
    previous: &Frame,
    time: i64,
    evidence: &mut Evidence,
) -> Result<Option<Value>, String> {
    let (node, port, instrument, sample) = match operand {
        Operand::Number { value } => return Ok(Some(serde_json::json!(value))),
        Operand::Boolean { value } => return Ok(Some(Value::Bool(*value))),
        Operand::Output {
            node,
            port,
            instrument,
            sample,
        } => (node, port, instrument, sample),
    };
    let interval = plan
        .request
        .nodes
        .iter()
        .find(|n| &n.graph_node_id == node)
        .and_then(|n| n.context.interval_ms)
        .and_then(|v| i64::try_from(v).ok())
        .ok_or("data_strategy_interval_invalid")?;
    let (frame, expected) = match sample {
        Sample::Current => (current, time),
        Sample::Previous => (
            previous,
            time.checked_sub(interval)
                .ok_or("data_strategy_time_invalid")?,
        ),
    };
    let Some(response) = frame.get(node) else {
        return Ok(None);
    };
    let batch = response
        .outputs
        .get(port)
        .ok_or("data_strategy_output_missing")?;
    let points = batch
        .points
        .iter()
        .filter(|p| &p.instrument_id == instrument)
        .collect::<Vec<_>>();
    let point: &DataPoint = match points.as_slice() {
        [point] => point,
        _ => return Err("data_strategy_output_cardinality_invalid".into()),
    };
    if !point.provenance.completed || point.provenance.event_end_ms != expected {
        return Err("data_strategy_output_time_mismatch".into());
    }
    evidence.known = evidence.known.max(point.provenance.known_at_ms);
    evidence.arrived = evidence.arrived.max(point.provenance.arrived_at_ms);
    evidence
        .hashes
        .insert(point.provenance.content_sha256.clone());
    match &point.result {
        PointValue::WarmingUp => Ok(None),
        PointValue::Ready(value)
            if value.is_boolean() || value.as_f64().is_some_and(f64::is_finite) =>
        {
            Ok(Some(value.clone()))
        }
        PointValue::Ready(_) => Err("data_strategy_output_type_invalid".into()),
    }
}
fn eval_rule(
    rule: &Rule,
    plan: &CompiledDataPlan,
    current: &Frame,
    previous: &Frame,
    time: i64,
    evidence: &mut Evidence,
) -> Result<Option<bool>, String> {
    // Evaluate every branch: missing data and errors never become false via a
    // boolean short circuit, including under NOT or ANY.
    match rule {
        Rule::Compare { left, op, right } => {
            let left = operand_value(left, plan, current, previous, time, evidence)?;
            let right = operand_value(right, plan, current, previous, time, evidence)?;
            match (left, right) {
                (Some(l), Some(r)) => {
                    if let (Some(l), Some(r)) = (l.as_f64(), r.as_f64()) {
                        Ok(Some(match op {
                            Comparison::Greater => l > r,
                            Comparison::GreaterEqual => l >= r,
                            Comparison::Less => l < r,
                            Comparison::LessEqual => l <= r,
                            Comparison::Equal => l == r,
                        }))
                    } else if matches!(op, Comparison::Equal) && l.is_boolean() && r.is_boolean() {
                        Ok(Some(l == r))
                    } else {
                        Err("data_strategy_comparison_type_invalid".into())
                    }
                }
                _ => Ok(None),
            }
        }
        Rule::IsTrue { value } => operand_value(value, plan, current, previous, time, evidence)?
            .map(|v| {
                v.as_bool()
                    .ok_or_else(|| "data_strategy_boolean_required".into())
            })
            .transpose(),
        Rule::Not { rule } => {
            Ok(eval_rule(rule, plan, current, previous, time, evidence)?.map(|v| !v))
        }
        Rule::All { rules } | Rule::Any { rules } => {
            let values = rules
                .iter()
                .map(|rule| eval_rule(rule, plan, current, previous, time, evidence))
                .collect::<Result<Vec<_>, _>>()?;
            if values.iter().any(Option::is_none) {
                return Ok(None);
            }
            Ok(Some(if matches!(rule, Rule::All { .. }) {
                values.iter().all(|v| v == &Some(true))
            } else {
                values.iter().any(|v| v == &Some(true))
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_pipeline::{compile_data_plan, tests::fixture};
    use serde_json::json;
    use tradeassembly_plugin_sdk::{data_contract::*, data_evaluation::*};
    fn setup() -> (DataStrategy, CompiledDataPlan) {
        let (graph, mut request, contracts) = fixture();
        request.trade_instruments = vec![InstrumentContext {
            instrument_id: "ETH/USD".into(),
            asset_class: "crypto".into(),
            instrument_family: "crypto_asset".into(),
        }];
        let plan = compile_data_plan(&graph, request, contracts).unwrap();
        let output = |sample| Operand::Output {
            node: "result".into(),
            port: "value".into(),
            instrument: "BTC/USD".into(),
            sample,
        };
        let rule = |sample, op| Rule::Compare {
            left: output(sample),
            op,
            right: Operand::Number { value: 5.0 },
        };
        let program = DataStrategy {
            version: 1,
            nodes: plan.request.nodes.clone(),
            bindings: plan.request.bindings.clone(),
            trade_instruments: plan.request.trade_instruments.clone(),
            entry: Rule::All {
                rules: vec![
                    rule(Sample::Previous, Comparison::LessEqual),
                    rule(Sample::Current, Comparison::Greater),
                ],
            },
            exit: Rule::IsTrue {
                value: Operand::Boolean { value: false },
            },
        };
        (program, plan)
    }
    fn frame(value: PointValue, end: i64, known: i64) -> Frame {
        let p = DataPoint {
            instrument_id: "BTC/USD".into(),
            source_event_id: end.to_string(),
            provenance: BatchProvenance {
                content_sha256: end.to_string(),
                upstream_content_sha256: vec![],
                event_start_ms: end - 86_400_000,
                event_end_ms: end,
                known_at_ms: known,
                arrived_at_ms: known,
                completed: true,
                source_revision: "1".into(),
            },
            result: value,
        };
        BTreeMap::from([(
            "result".into(),
            DataEvaluationResponse {
                schema_version: EvaluationVersion::V1,
                outputs: BTreeMap::from([(
                    "value".into(),
                    DataBatch {
                        schema: SchemaIdentity {
                            schema_ref: "test".into(),
                            sha256: "a".repeat(64),
                        },
                        shape: DataShape::Series,
                        points: vec![p],
                    },
                )]),
                state: BTreeMap::new(),
            },
        )])
    }
    #[test]
    fn data_strategy_crossing_is_strict_current_inclusive_previous_and_preserves_context_causality()
    {
        let (program, plan) = setup();
        let day = 86_400_000;
        let prior = frame(PointValue::Ready(json!(5)), day, 3 * day);
        let current = frame(PointValue::Ready(json!(6)), 2 * day, 2 * day);
        let signal = program
            .evaluate(&plan, &current, &prior, 2 * day, 3 * day)
            .unwrap();
        assert_eq!(signal.entry, SignalValue::Ready { value: true });
        assert_eq!(signal.known_at_ms, 3 * day);
        assert_eq!(signal.trade_instruments[0].instrument_id, "ETH/USD");
        assert_eq!(
            program
                .evaluate(&plan, &current, &prior, 2 * day, 2 * day)
                .unwrap_err(),
            "data_strategy_not_yet_known"
        );
        assert_eq!(
            program
                .evaluate(
                    &plan,
                    &frame(PointValue::Ready(json!(5)), 2 * day, 2 * day),
                    &prior,
                    2 * day,
                    3 * day
                )
                .unwrap()
                .entry,
            SignalValue::Ready { value: false }
        );
    }
    #[test]
    fn data_strategy_missing_warmup_bad_types_and_gaps_never_convert_to_false() {
        let (mut program, plan) = setup();
        let day = 86_400_000;
        let current = frame(PointValue::Ready(json!(6)), 2 * day, 2 * day);
        assert_eq!(
            program
                .evaluate(&plan, &current, &Frame::new(), 2 * day, 2 * day)
                .unwrap()
                .entry,
            SignalValue::WarmingUp
        );
        let warm = frame(PointValue::WarmingUp, day, day);
        assert_eq!(
            program
                .evaluate(&plan, &current, &warm, 2 * day, 2 * day)
                .unwrap()
                .entry,
            SignalValue::WarmingUp
        );
        let bad = frame(PointValue::Ready(json!("bad")), day, day);
        assert_eq!(
            program
                .evaluate(&plan, &current, &bad, 2 * day, 2 * day)
                .unwrap_err(),
            "data_strategy_output_type_invalid"
        );
        let stale = frame(PointValue::Ready(json!(4)), day / 2, day / 2);
        assert_eq!(
            program
                .evaluate(&plan, &current, &stale, 2 * day, 2 * day)
                .unwrap_err(),
            "data_strategy_output_time_mismatch"
        );
        program.entry = Rule::Any {
            rules: vec![
                Rule::IsTrue {
                    value: Operand::Boolean { value: true },
                },
                program.entry,
            ],
        };
        assert_eq!(
            program
                .evaluate(&plan, &current, &Frame::new(), 2 * day, 2 * day)
                .unwrap()
                .entry,
            SignalValue::WarmingUp
        );
        program.entry = Rule::Not {
            rule: Box::new(program.entry),
        };
        assert_eq!(
            program
                .evaluate(&plan, &current, &Frame::new(), 2 * day, 2 * day)
                .unwrap()
                .entry,
            SignalValue::WarmingUp
        );
    }
    #[test]
    fn data_strategy_cannot_be_ignored_by_legacy_price_compiler() {
        let mut spec: Value = serde_json::from_str(include_str!(
            "../../examples/strategy-spec/v3/valid/static-equity.json"
        ))
        .unwrap();
        spec["stages"]["evaluate"]["substeps"][0]["config"] = json!({"tradeassembly.expression":"bar.close > 0","tradeassembly.quantity":1_000_000,"tradeassembly.data_strategy":{}});
        assert!(crate::strategy_kernel::CompiledStrategy::compile_json(&spec).is_err());
        assert!(
            crate::strategy_kernel::portfolio_program::ResearchProgram::compile_json(&spec)
                .is_err()
        );
    }
    #[test]
    fn data_strategy_rejects_changed_plan_context_and_unbounded_rules() {
        let (mut program, plan) = setup();
        program.nodes[0].parameters = json!({"changed":true});
        assert_eq!(
            program.validate(&plan).unwrap_err(),
            "data_strategy_plan_mismatch"
        );
        program.nodes = plan.request.nodes.clone();
        for _ in 0..10 {
            program.entry = Rule::Not {
                rule: Box::new(program.entry),
            };
        }
        assert_eq!(
            program.validate(&plan).unwrap_err(),
            "data_strategy_rule_limit"
        );
    }
}
