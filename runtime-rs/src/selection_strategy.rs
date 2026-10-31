//! Explicit selected-subject strategy templates. Calculation remains in plugins.
use crate::{
    data_pipeline::*,
    data_strategy::{Comparison, DataStrategy, Operand, Rule, Sample},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CONFIG_KEY: &str = "tradeassembly.selection_strategy";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectorRef {
    pub market_requirement_ref: String,
    pub selector_ref: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ContextTemplate {
    Subject {
        interval_ms: Option<u64>,
    },
    Selected {
        selection_ref: String,
        interval_ms: Option<u64>,
    },
    Literal {
        context: DataContext,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeTemplate {
    pub graph_node_id: String,
    pub parameters: Value,
    pub context: ContextTemplate,
    pub input_contexts: BTreeMap<String, ContextTemplate>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum InstrumentRef {
    Subject,
    Literal { instrument_id: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperandTemplate {
    Output {
        node: String,
        port: String,
        instrument: InstrumentRef,
        sample: Sample,
    },
    Number {
        value: f64,
    },
    Boolean {
        value: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuleTemplate {
    Compare {
        left: OperandTemplate,
        op: Comparison,
        right: OperandTemplate,
    },
    IsTrue {
        value: OperandTemplate,
    },
    All {
        rules: Vec<RuleTemplate>,
    },
    Any {
        rules: Vec<RuleTemplate>,
    },
    Not {
        rule: Box<RuleTemplate>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionStrategy {
    pub schema_version: String,
    pub selections: BTreeMap<String, SelectorRef>,
    pub trade_selection_ref: String,
    pub nodes: Vec<NodeTemplate>,
    pub bindings: Vec<PortBinding>,
    pub entry: RuleTemplate,
    pub exit: RuleTemplate,
}
fn operand(t: &OperandTemplate, subject: &str) -> Operand {
    match t {
        OperandTemplate::Output {
            node,
            port,
            instrument,
            sample,
        } => Operand::Output {
            node: node.clone(),
            port: port.clone(),
            instrument: match instrument {
                InstrumentRef::Subject => subject.into(),
                InstrumentRef::Literal { instrument_id } => instrument_id.clone(),
            },
            sample: sample.clone(),
        },
        OperandTemplate::Number { value } => Operand::Number { value: *value },
        OperandTemplate::Boolean { value } => Operand::Boolean { value: *value },
    }
}
fn rule(t: &RuleTemplate, subject: &str, depth: usize, count: &mut usize) -> Result<Rule, String> {
    *count += 1;
    if depth > 8 || *count > 64 {
        return Err("selection_strategy_rule_limit".into());
    }
    Ok(match t {
        RuleTemplate::Compare { left, op, right } => Rule::Compare {
            left: operand(left, subject),
            op: op.clone(),
            right: operand(right, subject),
        },
        RuleTemplate::IsTrue { value } => Rule::IsTrue {
            value: operand(value, subject),
        },
        RuleTemplate::All { rules } => Rule::All {
            rules: rules
                .iter()
                .map(|r| rule(r, subject, depth + 1, count))
                .collect::<Result<_, _>>()?,
        },
        RuleTemplate::Any { rules } => Rule::Any {
            rules: rules
                .iter()
                .map(|r| rule(r, subject, depth + 1, count))
                .collect::<Result<_, _>>()?,
        },
        RuleTemplate::Not { rule: r } => Rule::Not {
            rule: Box::new(rule(r, subject, depth + 1, count)?),
        },
    })
}
impl SelectionStrategy {
    pub fn resolve(
        &self,
        selected: &BTreeMap<String, Vec<InstrumentContext>>,
        subject: &str,
    ) -> Result<DataStrategy, String> {
        if self.schema_version != "tradeassembly.selection_strategy.v1"
            || self.selections.is_empty()
            || self.selections.len() > 16
            || self.nodes.is_empty()
            || self.nodes.len() > 32
            || selected.keys().ne(self.selections.keys())
        {
            return Err("selection_strategy_invalid".into());
        }
        let subject = selected
            .get(&self.trade_selection_ref)
            .and_then(|s| s.iter().find(|s| s.instrument_id == subject))
            .ok_or("selection_strategy_subject_not_selected")?
            .clone();
        let context = |c: &ContextTemplate| -> Result<DataContext, String> {
            Ok(match c {
                ContextTemplate::Subject { interval_ms } => DataContext {
                    instruments: vec![subject.clone()],
                    interval_ms: *interval_ms,
                },
                ContextTemplate::Selected {
                    selection_ref,
                    interval_ms,
                } => DataContext {
                    instruments: selected
                        .get(selection_ref)
                        .ok_or("selection_strategy_context_missing")?
                        .clone(),
                    interval_ms: *interval_ms,
                },
                ContextTemplate::Literal { context } => context.clone(),
            })
        };
        let nodes = self
            .nodes
            .iter()
            .map(|n| {
                Ok(PipelineNode {
                    graph_node_id: n.graph_node_id.clone(),
                    parameters: n.parameters.clone(),
                    context: context(&n.context)?,
                    input_contexts: n
                        .input_contexts
                        .iter()
                        .map(|(k, v)| Ok((k.clone(), context(v)?)))
                        .collect::<Result<_, String>>()?,
                })
            })
            .collect::<Result<_, String>>()?;
        let mut count = 0;
        Ok(DataStrategy {
            version: 1,
            nodes,
            bindings: self.bindings.clone(),
            trade_instruments: vec![subject.clone()],
            entry: rule(&self.entry, &subject.instrument_id, 0, &mut count)?,
            exit: rule(&self.exit, &subject.instrument_id, 0, &mut count)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_template_version_does_not_extend_the_legacy_wire() {
        let value = serde_json::json!({"schemaVersion":"tradeassembly.selection_strategy.v1","selections":{"primary":{"marketRequirementRef":"primary","selectorRef":"symbol"}},"tradeSelectionRef":"primary","nodes":[{"graphNodeId":"n","parameters":{},"context":{"kind":"subject","intervalMs":86400000},"inputContexts":{}}],"bindings":[],"entry":{"kind":"is_true","value":{"kind":"boolean","value":true}},"exit":{"kind":"is_true","value":{"kind":"boolean","value":false}}});
        assert!(serde_json::from_value::<DataStrategy>(value.clone()).is_err());
        let mut template: SelectionStrategy = serde_json::from_value(value).unwrap();
        let selected = BTreeMap::from([(
            "primary".into(),
            vec![InstrumentContext {
                instrument_id: "A".into(),
                asset_class: "equity".into(),
                instrument_family: "equity".into(),
            }],
        )]);
        assert_eq!(
            template.resolve(&selected, "A").unwrap().trade_instruments[0].instrument_id,
            "A"
        );
        template.schema_version = "tradeassembly.selection_strategy.v2".into();
        assert_eq!(
            template.resolve(&selected, "A").unwrap_err(),
            "selection_strategy_invalid"
        );
    }
}
