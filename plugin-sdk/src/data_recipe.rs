//! Closed declarative recipe documents. They describe a bounded composition of
//! admitted native operations; they are never executable code or authority.
use crate::{data_contract::DataPort, data_package::metadata_budget};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_RECIPE_NODES: usize = 32;
pub const MAX_RECIPE_PORTS: usize = 64;
const MAX_CHOICE_DEPTH: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecipeVersion {
    #[serde(rename = "tradeassembly.data_recipe.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataRecipe {
    pub schema_version: RecipeVersion,
    pub recipe_id: String,
    pub semantics_version: String,
    pub parameter_schema: Value,
    pub inputs: BTreeMap<String, DataPort>,
    pub nodes: BTreeMap<String, RecipeNode>,
    pub bindings: Vec<RecipeBinding>,
    pub outputs: BTreeMap<String, RecipeOutput>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeNode {
    pub operation: RecipeOperation,
    pub parameters: BTreeMap<String, RecipeParameter>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeOperation {
    /// `self` means this exact verified package, not a registry search.
    pub plugin_ref: String,
    pub operation_id: String,
    pub semantics_version: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecipeParameter {
    Constant { value: Value },
    Parameter { name: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecipePort {
    Input {
        port: String,
    },
    Output {
        node: String,
        port: String,
    },
    /// A bounded parameter-only branch permits zero-delay pass-through without
    /// invoking an operation outside its admitted parameter domain. There are no
    /// expressions, scripts, lookups, loops or data-dependent hidden invocations.
    Choose {
        parameter: String,
        equals: Value,
        when_true: Box<RecipePort>,
        when_false: Box<RecipePort>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeBinding {
    pub producer: RecipePort,
    pub consumer_node: String,
    pub input_port: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeOutput {
    pub producer: RecipePort,
    pub data: DataPort,
}
fn name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
fn reference(s: &str) -> bool {
    !s.is_empty() && s.len() <= 256 && s.chars().all(|c| !c.is_control() && !c.is_whitespace())
}
fn normalized(value: &Value) -> Result<Value, &'static str> {
    let mut value = value.clone();
    crate::canonical::normalize_numbers(&mut value)
        .map_err(|_| "data_recipe_numeric_range_invalid")?;
    Ok(value)
}
impl DataRecipe {
    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > crate::data_package::MAX_METADATA_DOCUMENT_BYTES {
            return Err("data_recipe_document_too_large");
        }
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| "data_recipe_document_invalid")?;
        metadata_budget(&value)?;
        let recipe: Self =
            serde_json::from_value(value).map_err(|_| "data_recipe_document_invalid")?;
        recipe.validate_structure()?;
        Ok(recipe)
    }
    /// Structural validation is separate from resolving exact operation contracts,
    /// schema compatibility and contextual data availability at host preparation.
    pub fn validate_structure(&self) -> Result<(), &'static str> {
        metadata_budget(&serde_json::to_value(self).map_err(|_| "data_recipe_document_invalid")?)?;
        if !name(&self.recipe_id)
            || !reference(&self.semantics_version)
            || self.nodes.len() > MAX_RECIPE_NODES
            || self.inputs.len() > MAX_RECIPE_PORTS
            || self.outputs.is_empty()
            || self.outputs.len() > MAX_RECIPE_PORTS
            || self.bindings.len() > MAX_RECIPE_NODES * MAX_RECIPE_PORTS
        {
            return Err("data_recipe_bounds_invalid");
        }
        crate::data_package::schema_budget(&self.parameter_schema)?;
        if !self
            .inputs
            .keys()
            .chain(self.outputs.keys())
            .chain(self.nodes.keys())
            .all(|id| name(id))
        {
            return Err("data_recipe_name_invalid");
        }
        let parameter_exists = |id: &str| {
            name(id)
                && self
                    .parameter_schema
                    .get("properties")
                    .and_then(|p| p.get(id))
                    .is_some()
        };
        for node in self.nodes.values() {
            if !reference(&node.operation.plugin_ref)
                || !reference(&node.operation.operation_id)
                || !reference(&node.operation.semantics_version)
                || node.parameters.len() > MAX_RECIPE_PORTS
            {
                return Err("data_recipe_operation_invalid");
            }
            for (key, binding) in &node.parameters {
                if !name(key) {
                    return Err("data_recipe_name_invalid");
                }
                match binding {
                    RecipeParameter::Parameter { name } if !parameter_exists(name) => {
                        return Err("data_recipe_parameter_missing")
                    }
                    RecipeParameter::Constant { value } => {
                        normalized(value)?;
                    }
                    _ => {}
                }
            }
        }
        let mut dependencies = BTreeMap::<String, BTreeSet<String>>::new();
        let mut bound = BTreeSet::new();
        for binding in &self.bindings {
            if !self.nodes.contains_key(&binding.consumer_node)
                || !name(&binding.input_port)
                || !bound.insert((&binding.consumer_node, &binding.input_port))
            {
                return Err("data_recipe_input_binding_invalid");
            }
            let nodes = self.check_port(&binding.producer, 0)?;
            dependencies
                .entry(binding.consumer_node.clone())
                .or_default()
                .extend(nodes);
        }
        for output in self.outputs.values() {
            self.check_port(&output.producer, 0)?;
        }
        fn visit<'a>(
            node: &'a str,
            dependencies: &'a BTreeMap<String, BTreeSet<String>>,
            active: &mut BTreeSet<&'a str>,
            done: &mut BTreeSet<&'a str>,
        ) -> Result<(), &'static str> {
            if done.contains(node) {
                return Ok(());
            }
            if !active.insert(node) {
                return Err("data_recipe_cycle");
            }
            for parent in dependencies.get(node).into_iter().flatten() {
                visit(parent, dependencies, active, done)?;
            }
            active.remove(node);
            done.insert(node);
            Ok(())
        }
        let mut done = BTreeSet::new();
        for node in self.nodes.keys() {
            visit(node, &dependencies, &mut BTreeSet::new(), &mut done)?;
        }
        Ok(())
    }
    fn check_port(
        &self,
        port: &RecipePort,
        depth: usize,
    ) -> Result<BTreeSet<String>, &'static str> {
        if depth > MAX_CHOICE_DEPTH {
            return Err("data_recipe_choice_bounds_invalid");
        }
        match port {
            RecipePort::Input { port } if self.inputs.contains_key(port) => Ok(BTreeSet::new()),
            RecipePort::Output { node, port } if self.nodes.contains_key(node) && name(port) => {
                Ok(BTreeSet::from([node.clone()]))
            }
            RecipePort::Choose {
                parameter,
                equals,
                when_true,
                when_false,
            } => {
                if !name(parameter)
                    || self
                        .parameter_schema
                        .get("properties")
                        .and_then(|p| p.get(parameter))
                        .is_none()
                {
                    return Err("data_recipe_parameter_missing");
                }
                normalized(equals)?;
                let mut nodes = self.check_port(when_true, depth + 1)?;
                nodes.extend(self.check_port(when_false, depth + 1)?);
                Ok(nodes)
            }
            _ => Err("data_recipe_port_missing"),
        }
    }
    /// Only normalized, schema-validated host parameters may reach expansion.
    pub fn resolve_port(
        &self,
        port: &RecipePort,
        parameters: &Value,
    ) -> Result<RecipePort, &'static str> {
        fn resolve(
            port: &RecipePort,
            parameters: &Value,
            depth: usize,
        ) -> Result<RecipePort, &'static str> {
            if depth > MAX_CHOICE_DEPTH {
                return Err("data_recipe_choice_bounds_invalid");
            }
            match port {
                RecipePort::Choose {
                    parameter,
                    equals,
                    when_true,
                    when_false,
                } => {
                    let value = parameters
                        .get(parameter)
                        .ok_or("data_recipe_parameter_missing")?;
                    resolve(
                        if normalized(value)? == normalized(equals)? {
                            when_true
                        } else {
                            when_false
                        },
                        parameters,
                        depth + 1,
                    )
                }
                _ => Ok(port.clone()),
            }
        }
        self.check_port(port, 0)?;
        resolve(port, parameters, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> Value {
        let port = json!({"schema":{"schemaRef":"schema://test/series@1","sha256":"a".repeat(64)},"shape":"series","time":{"intervalMs":1,"completedOnly":true}});
        json!({"schemaVersion":"tradeassembly.data_recipe.v1","recipeId":"delay-or-current","semanticsVersion":"1", "parameterSchema":{"type":"object","properties":{"period":{"type":"integer","minimum":0}},"required":["period"],"additionalProperties":false},"inputs":{"source":port.clone()},"nodes":{"delay":{"operation":{"pluginRef":"self","operationId":"data.delay","semanticsVersion":"1"},"parameters":{"period":{"kind":"parameter","name":"period"}}}},"bindings":[{"producer":{"kind":"input","port":"source"},"consumerNode":"delay","inputPort":"source"}],"outputs":{"value":{"data":port,"producer":{"kind":"choose","parameter":"period","equals":0,"when_true":{"kind":"input","port":"source"},"when_false":{"kind":"output","node":"delay","port":"value"}}}}})
    }
    fn decode(value: &Value) -> Result<DataRecipe, &'static str> {
        DataRecipe::decode(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn recipe_zero_delay_is_explicit_and_numeric_branches_are_canonical() {
        let recipe = decode(&fixture()).unwrap();
        let output = &recipe.outputs["value"].producer;
        assert_eq!(
            recipe.resolve_port(output, &json!({"period":0.0})).unwrap(),
            RecipePort::Input {
                port: "source".into()
            }
        );
        assert_eq!(
            recipe.resolve_port(output, &json!({"period":2})).unwrap(),
            RecipePort::Output {
                node: "delay".into(),
                port: "value".into()
            }
        );
        assert_eq!(
            recipe.resolve_port(output, &json!({})).unwrap_err(),
            "data_recipe_parameter_missing"
        );
    }
    #[test]
    fn recipe_rejects_cycles_even_in_an_unselected_branch() {
        let mut value = fixture();
        value["bindings"][0]["producer"] = json!({"kind":"output","node":"delay","port":"value"});
        assert_eq!(decode(&value).unwrap_err(), "data_recipe_cycle");
    }
    #[test]
    fn recipe_rejects_dangling_ports_duplicate_inputs_and_undefined_parameters() {
        for case in 0..4 {
            let mut value = fixture();
            match case {
                0 => value["bindings"][0]["producer"]["port"] = json!("missing"),
                1 => {
                    let duplicate = value["bindings"][0].clone();
                    value["bindings"].as_array_mut().unwrap().push(duplicate);
                }
                2 => value["nodes"]["delay"]["parameters"]["period"]["name"] = json!("unknown"),
                _ => value["outputs"]["value"]["producer"]["when_false"]["node"] = json!("missing"),
            }
            assert!(decode(&value).is_err(), "{case}");
        }
    }
    #[test]
    fn recipe_rejects_code_unknown_versions_and_unsupported_nested_nodes() {
        for case in 0..4 {
            let mut value = fixture();
            match case {
                0 => value["schemaVersion"] = json!("tradeassembly.data_recipe.v2"),
                1 => value["python"] = json!("not executable"),
                2 => {
                    value["nodes"]["delay"]["parameters"]["period"] =
                        json!({"kind":"expression","source":"period+1"})
                }
                _ => value["nodes"]["delay"]["recipeRef"] = json!("hidden-dependency"),
            }
            assert_eq!(decode(&value).unwrap_err(), "data_recipe_document_invalid");
        }
    }
    #[test]
    fn recipe_bounds_work_and_rejects_lossy_numeric_constants() {
        let mut value = fixture();
        value["nodes"]["delay"]["parameters"]["period"] =
            json!({"kind":"constant","value":9_007_199_254_740_993_u64});
        assert_eq!(
            decode(&value).unwrap_err(),
            "data_recipe_numeric_range_invalid"
        );
        let mut value = fixture();
        let node = value["nodes"]["delay"].clone();
        for n in 0..MAX_RECIPE_NODES {
            value["nodes"][format!("extra{n}")] = node.clone();
        }
        assert_eq!(decode(&value).unwrap_err(), "data_recipe_bounds_invalid");
    }
}
