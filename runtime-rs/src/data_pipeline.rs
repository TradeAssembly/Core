//! Pure compilation over an already resolved capability graph. No invocation,
//! credential access, provider fallback, calculation or checkpoint migration.

use crate::ports::capability::CapabilityGraphResolution;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tradeassembly_plugin_sdk::data_contract::*;

#[cfg(test)]
#[path = "data_pipeline_tests.rs"]
pub(crate) mod tests;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstrumentContext {
    pub instrument_id: String,
    pub asset_class: String,
    pub instrument_family: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DataContext {
    pub instruments: Vec<InstrumentContext>,
    pub interval_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PipelineNode {
    /// Exact node ID in the capability graph, never a new plugin registry key.
    pub graph_node_id: String,
    pub parameters: Value,
    pub context: DataContext,
    /// Explicit context inputs may differ from output/traded instruments.
    pub input_contexts: BTreeMap<String, DataContext>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortBinding {
    pub producer_node: String,
    pub output_port: String,
    pub consumer_node: String,
    pub input_port: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PipelineOutput {
    pub node: String,
    pub port: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PipelineRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_outputs: Vec<PipelineOutput>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source_captures: BTreeMap<String, String>,
    pub capability_graph_revision: String,
    pub mode: String,
    /// Recorded for identity only: data readiness does not grant trade eligibility.
    pub trade_instruments: Vec<InstrumentContext>,
    pub nodes: Vec<PipelineNode>,
    pub bindings: Vec<PortBinding>,
}

/// Loaded by the host from a verified installed package. This is not an
/// install/admission API and must never be populated from untrusted request JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedDataContract {
    pub graph_node_id: String,
    pub manifest_fingerprint: String,
    pub contract_sha256: String,
    pub contract: tradeassembly_plugin_sdk::data_history::VersionedOperationData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanDiagnosticCode {
    GraphBlocked,
    InvalidContract,
    UnresolvedNode,
    DuplicateNode,
    InvalidParameters,
    UnsupportedContext,
    MissingInput,
    DuplicateInput,
    UnknownPort,
    UndeclaredDependency,
    IncompatibleSchema,
    IncompatibleTimeContext,
    Cycle,
    WarmupOverflow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanDiagnostic {
    pub code: PlanDiagnosticCode,
    pub node_id: Option<String>,
    pub port: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompiledDataPlan {
    pub schema_version: DataPlanVersion,
    pub capability_graph_fingerprint: String,
    pub evaluation_epoch: String,
    pub request: PipelineRequest,
    pub contracts: Vec<VerifiedDataContract>,
    /// Producer-first deterministic order. Does not authorize invocation.
    pub execution_order: Vec<String>,
    /// Source observations required on the longest required path. V1 requires
    /// matching intervals; resampling must be an explicit future operation.
    pub source_warmup_observations: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source_history_by_output: BTreeMap<String, BTreeMap<String, BTreeMap<String, u64>>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub selected_source_history: BTreeMap<String, u64>,
    pub plan_sha256: String,
}

pub use tradeassembly_plugin_sdk::canonical::canonical_sha256;
use tradeassembly_plugin_sdk::canonical::{normalize_numbers, MAX_EXACT_INTEGER};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataPlanVersion {
    #[serde(rename = "tradeassembly.compiled_data_plan.v1")]
    V1,
    #[serde(rename = "tradeassembly.compiled_data_plan.v2")]
    V2,
    #[serde(rename = "tradeassembly.compiled_data_plan.v3")]
    V3,
    #[serde(rename = "tradeassembly.compiled_data_plan.v4")]
    V4,
    #[serde(rename = "tradeassembly.compiled_data_plan.v5")]
    V5,
}

fn issue(
    code: PlanDiagnosticCode,
    node: Option<&str>,
    port: Option<&str>,
    message: impl Into<String>,
) -> PlanDiagnostic {
    PlanDiagnostic {
        code,
        node_id: node.map(str::to_owned),
        port: port.map(str::to_owned),
        message: message.into(),
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_schema(value: &SchemaIdentity) -> bool {
    !value.schema_ref.trim().is_empty() && valid_digest(&value.sha256)
}

pub(crate) fn local_schema_refs(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().all(|(key, value)| {
            if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef") {
                value.as_str().is_some_and(|r| r.starts_with('#'))
            } else {
                local_schema_refs(value)
            }
        }),
        Value::Array(values) => values.iter().all(local_schema_refs),
        _ => true,
    }
}

pub(crate) fn validate_contract(contract: &OperationDataContract) -> bool {
    !contract.operation_id.trim().is_empty()
        && !contract.semantics_version.trim().is_empty()
        && !contract.outputs.is_empty()
        && contract.warmup.minimum_observations > 0
        && !contract.applicability.asset_classes.is_empty()
        && !contract.applicability.instrument_families.is_empty()
        && !contract.applicability.modes.is_empty()
        && contract
            .inputs
            .iter()
            .all(|(name, port)| !name.trim().is_empty() && !port.dependency_id.trim().is_empty())
        && contract.outputs.keys().all(|name| !name.trim().is_empty())
        && contract
            .inputs
            .values()
            .map(|port| &port.data)
            .chain(contract.outputs.values())
            .all(|port| valid_schema(&port.schema) && port.time.interval_ms != Some(0))
        && contract
            .state
            .as_ref()
            .is_none_or(|state| valid_schema(&state.schema) && !state.version.trim().is_empty())
        && contract.parameter_schema.is_object()
        && local_schema_refs(&contract.parameter_schema)
}

fn valid_context(context: &DataContext) -> bool {
    !context.instruments.is_empty()
        && context.interval_ms != Some(0)
        && context.instruments.iter().all(|i| {
            !i.instrument_id.trim().is_empty()
                && !i.asset_class.is_empty()
                && !i.instrument_family.is_empty()
        })
        && context
            .instruments
            .iter()
            .map(|i| &i.instrument_id)
            .collect::<BTreeSet<_>>()
            .len()
            == context.instruments.len()
}

/// Canonicalize unordered pipeline definitions consistently for compilation and
/// immutable strategy binding. Retains duplicate definitions for validation.
pub(crate) fn normalize_request(request: &mut PipelineRequest) -> Vec<PlanDiagnostic> {
    use PlanDiagnosticCode::*;
    let mut errors = Vec::new();
    request.selected_outputs.sort();
    request
        .nodes
        .sort_by(|a, b| a.graph_node_id.cmp(&b.graph_node_id));
    for node in &mut request.nodes {
        if normalize_numbers(&mut node.parameters).is_err() {
            errors.push(issue(
                InvalidParameters,
                Some(&node.graph_node_id),
                None,
                "Numeric parameters must be finite and within the exact supported range ±(2^53−1).",
            ));
        }
        node.context
            .instruments
            .sort_by(|a, b| a.instrument_id.cmp(&b.instrument_id));
        for context in node.input_contexts.values_mut() {
            context
                .instruments
                .sort_by(|a, b| a.instrument_id.cmp(&b.instrument_id));
        }
    }
    request
        .trade_instruments
        .sort_by(|a, b| a.instrument_id.cmp(&b.instrument_id));
    request.bindings.sort_by(|a, b| {
        (
            &a.consumer_node,
            &a.input_port,
            &a.producer_node,
            &a.output_port,
        )
            .cmp(&(
                &b.consumer_node,
                &b.input_port,
                &b.producer_node,
                &b.output_port,
            ))
    });
    errors
}

/// Compiles explicit bindings backed by the selected dependency graph.
/// Schema identity is exact; no implicit shape conversion or resampling.
pub fn compile_data_plan(
    graph: &CapabilityGraphResolution,
    mut request: PipelineRequest,
    mut contracts: Vec<VerifiedDataContract>,
) -> Result<CompiledDataPlan, Vec<PlanDiagnostic>> {
    use PlanDiagnosticCode::*;
    let mut errors = Vec::new();
    if !graph.ok
        || graph.graph_fingerprint.is_empty()
        || request.capability_graph_revision.trim().is_empty()
        || !graph.no_silent_fallback
        || !graph.blockers.is_empty()
    {
        let mut diagnostics = vec![issue(
            GraphBlocked,
            None,
            None,
            "Resolve and save an admitted capability graph before compiling data bindings.",
        )];
        diagnostics.extend(graph.blockers.iter().map(|blocker| {
            issue(
                GraphBlocked,
                blocker.subject.as_deref(),
                None,
                format!("{}: {}", blocker.code, blocker.message),
            )
        }));
        return Err(diagnostics);
    }
    errors.extend(normalize_request(&mut request));
    contracts.sort_by(|a, b| a.graph_node_id.cmp(&b.graph_node_id));
    for binding in &mut contracts {
        if normalize_numbers(&mut binding.contract.parameter_schema).is_err() {
            errors.push(issue(
                InvalidContract,
                Some(&binding.graph_node_id),
                None,
                "Parameter schema numbers exceed the exact supported numeric range.",
            ));
        }
    }
    let mut nodes = BTreeMap::new();
    for node in &request.nodes {
        if nodes.insert(node.graph_node_id.as_str(), node).is_some() {
            errors.push(issue(
                DuplicateNode,
                Some(&node.graph_node_id),
                None,
                "Each graph node must appear exactly once.",
            ));
        }
    }
    let mut metadata = BTreeMap::new();
    for binding in &contracts {
        if metadata
            .insert(binding.graph_node_id.as_str(), binding)
            .is_some()
            || !nodes.contains_key(binding.graph_node_id.as_str())
        {
            errors.push(issue(
                InvalidContract,
                Some(&binding.graph_node_id),
                None,
                "Contract must bind exactly one requested graph node.",
            ));
        }
    }
    let mut warmups = BTreeMap::new();
    for (id, node) in &nodes {
        let selected_node = graph.nodes.iter().find(|n| n.node_id == *id);
        let selected = selected_node.and_then(|n| {
            if n.blockers.is_empty() {
                n.selected.as_ref()
            } else {
                None
            }
        });
        let Some(selected) = selected else {
            errors.push(issue(
                UnresolvedNode,
                Some(id),
                None,
                "Select an admitted plugin operation through capability resolution.",
            ));
            continue;
        };
        let Some(binding) = metadata.get(id) else {
            errors.push(issue(
                InvalidContract,
                Some(id),
                None,
                "Install a verified data contract for the selected operation.",
            ));
            continue;
        };
        let contract = &binding.contract;
        if selected.manifest_fingerprint != binding.manifest_fingerprint
            || selected.operation_id != contract.operation_id
            || canonical_sha256(contract).ok().as_deref() != Some(binding.contract_sha256.as_str())
            || !validate_contract(contract)
            || !contract.inputs.values().all(|p| {
                selected
                    .operation
                    .traits
                    .input_schema_refs
                    .contains(&p.data.schema.schema_ref)
            })
            || !contract.outputs.values().all(|p| {
                selected
                    .operation
                    .traits
                    .output_schema_refs
                    .contains(&p.schema.schema_ref)
            })
        {
            errors.push(issue(
                InvalidContract,
                Some(id),
                None,
                "Contract identity, schemas and selected manifest/operation must agree.",
            ));
            continue;
        }
        if !valid_context(&node.context)
            || selected_node.unwrap().requirement.mode != request.mode
            || !contract.applicability.modes.contains(&request.mode)
            || !selected.operation.traits.modes.contains(&request.mode)
            || !node.context.instruments.iter().all(|i| {
                contract
                    .applicability
                    .asset_classes
                    .contains(&i.asset_class)
                    && contract
                        .applicability
                        .instrument_families
                        .contains(&i.instrument_family)
                    && selected
                        .operation
                        .traits
                        .instrument_families
                        .contains(&i.instrument_family)
            })
            || !contract
                .outputs
                .values()
                .all(|p| p.time.interval_ms == node.context.interval_ms)
            || node.input_contexts.iter().any(|(port, context)| {
                !contract.inputs.contains_key(port) || !valid_context(context)
            })
        {
            errors.push(issue(UnsupportedContext, Some(id), None, "Choose supported data instruments, mode and explicit input/output intervals. Broker eligibility is a separate check."));
        }
        if contract
            .inputs
            .values()
            .any(|input| input.data.time.interval_ms != node.context.interval_ms)
        {
            errors.push(issue(IncompatibleTimeContext, Some(id), None, "Operation input and output intervals must match in V1; temporal conversion requires a future unit-aware warmup contract."));
        }
        match jsonschema::validator_for(&contract.parameter_schema) {
            Ok(validator) if validator.is_valid(&node.parameters) => {}
            _ => errors.push(issue(
                InvalidParameters,
                Some(id),
                None,
                "Parameters must satisfy the pinned operation parameter schema.",
            )),
        }
        if let Some(v3) = contract.v3() {
            if contract.inputs.is_empty() && contract.state.is_some() {
                errors.push(issue(InvalidContract, Some(id), None,
                    "Stateful V3 source acquisition is unsupported; use a stateless source with explicit replay or a stateful downstream transform."));
            }
            for (output, inputs) in &v3.history.first_ready {
                for (input, expression) in inputs {
                    if let Err(reason) = expression.resolve(&node.parameters) {
                        errors.push(issue(
                            InvalidParameters,
                            Some(id),
                            Some(output),
                            format!("Input {input} history: {reason:?}"),
                        ));
                    }
                }
            }
            if let Err(reason) = v3.resolve(&node.parameters) {
                errors.push(issue(InvalidContract, Some(id), None, reason));
            }
        }
        let warmup = &contract.warmup;
        let parameter = match &warmup.observations_parameter {
            Some(name) => node
                .parameters
                .get(name)
                .and_then(Value::as_u64)
                .filter(|v| *v > 0),
            None => Some(0),
        };
        match parameter
            .and_then(|n| n.checked_add(warmup.additional_observations))
            .filter(|n| *n <= MAX_EXACT_INTEGER)
        {
            Some(n) => {
                warmups.insert(*id, n.max(warmup.minimum_observations));
            }
            None => errors.push(issue(
                InvalidParameters,
                Some(id),
                None,
                "Warmup requires a positive integer parameter and a bounded observation count.",
            )),
        }
    }
    // Do not inspect partial/invalid metadata further.
    if !errors.is_empty() {
        return Err(errors);
    }
    let mut incoming = BTreeMap::new();
    let mut dependencies: BTreeMap<&str, BTreeSet<&str>> =
        nodes.keys().map(|id| (*id, BTreeSet::new())).collect();
    for edge in &request.bindings {
        let (Some(producer), Some(consumer), Some(source_contract), Some(target_contract)) = (
            nodes.get(edge.producer_node.as_str()),
            nodes.get(edge.consumer_node.as_str()),
            metadata.get(edge.producer_node.as_str()),
            metadata.get(edge.consumer_node.as_str()),
        ) else {
            errors.push(issue(
                UnresolvedNode,
                Some(&edge.consumer_node),
                Some(&edge.input_port),
                "Both binding endpoints must be requested resolved nodes.",
            ));
            continue;
        };
        let (Some(output), Some(input)) = (
            source_contract.contract.outputs.get(&edge.output_port),
            target_contract.contract.inputs.get(&edge.input_port),
        ) else {
            errors.push(issue(
                UnknownPort,
                Some(&edge.consumer_node),
                Some(&edge.input_port),
                "Select declared input and output port names.",
            ));
            continue;
        };
        if incoming
            .insert(
                (edge.consumer_node.as_str(), edge.input_port.as_str()),
                edge.producer_node.as_str(),
            )
            .is_some()
        {
            errors.push(issue(
                DuplicateInput,
                Some(&edge.consumer_node),
                Some(&edge.input_port),
                "An input accepts one producer; use an explicit merge operation.",
            ));
        }
        dependencies
            .get_mut(edge.consumer_node.as_str())
            .unwrap()
            .insert(edge.producer_node.as_str());
        let consumer_graph_node = graph
            .nodes
            .iter()
            .find(|n| n.node_id == edge.consumer_node)
            .unwrap();
        let producer_graph_node = graph
            .nodes
            .iter()
            .find(|n| n.node_id == edge.producer_node)
            .unwrap();
        let consumer_selection = consumer_graph_node.selected.as_ref().unwrap();
        let producer_selection = producer_graph_node.selected.as_ref().unwrap();
        let declared = consumer_selection
            .operation
            .dependencies
            .iter()
            .find(|d| d.dependency_id == input.dependency_id);
        let declared_binding = declared.is_some_and(|dependency| {
            dependency.capability == producer_graph_node.requirement.capability
                && dependency
                    .operation_id
                    .as_ref()
                    .is_none_or(|id| id == &producer_selection.operation_id)
                && producer_graph_node.origin == "plugin_dependency"
                && producer_graph_node.parent_node_id.as_deref()
                    == Some(edge.consumer_node.as_str())
                && producer_graph_node.dependency_id.as_deref()
                    == Some(input.dependency_id.as_str())
                && graph.edges.iter().any(|e| {
                    e.from_node_id == edge.consumer_node
                        && e.to_node_id == edge.producer_node
                        && e.dependency_id == input.dependency_id
                        && e.origin == "plugin_dependency"
                        && e.required == dependency.required
                })
        });
        if matches!(
            target_contract.contract.schema_version,
            DataContractVersion::V1
        ) && !declared_binding
        {
            errors.push(issue(
                UndeclaredDependency,
                Some(&edge.consumer_node),
                Some(&edge.input_port),
                "Bind the selected operation's declared plugin dependency through its exact resolved child; strategy dependencies cannot supply this declaration.",
            ));
        }
        if output.schema != input.data.schema || output.shape != input.data.shape {
            errors.push(issue(IncompatibleSchema, Some(&edge.consumer_node), Some(&edge.input_port), "Producer and consumer require the same schema digest and shape; add an explicit conversion operation."));
        }
        let expected_context = consumer
            .input_contexts
            .get(&edge.input_port)
            .unwrap_or(&consumer.context);
        if &producer.context != expected_context
            || input.data.time.interval_ms != expected_context.interval_ms
            || output.time.interval_ms != input.data.time.interval_ms
            || (input.data.time.completed_only && !output.time.completed_only)
        {
            errors.push(issue(IncompatibleTimeContext, Some(&edge.consumer_node), Some(&edge.input_port), "Bind matching instruments and intervals with sufficient completion guarantees; no implicit resampling or symbol substitution."));
        }
    }
    for (id, binding) in &metadata {
        for (port, input) in &binding.contract.inputs {
            if input.required && !incoming.contains_key(&(*id, port.as_str())) {
                errors.push(issue(
                    MissingInput,
                    Some(id),
                    Some(port),
                    format!(
                        "Resolve dependency '{}' and bind an output supplying schema '{}'.",
                        input.dependency_id, input.data.schema.schema_ref
                    ),
                ));
            }
        }
    }
    let mut selected = BTreeSet::new();
    if request.selected_outputs.len() > 256 {
        errors.push(issue(
            InvalidContract,
            None,
            None,
            "Select at most 256 output ports.",
        ));
    }
    for output in &request.selected_outputs {
        if !selected.insert(output)
            || !metadata
                .get(output.node.as_str())
                .is_some_and(|binding| binding.contract.outputs.contains_key(&output.port))
        {
            errors.push(issue(
                UnknownPort,
                Some(&output.node),
                Some(&output.port),
                "Select each declared node/output port at most once.",
            ));
        }
    }
    let mut remaining = dependencies.clone();
    let mut order = Vec::new();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .find_map(|(id, deps)| deps.is_empty().then_some(*id));
        let Some(ready) = ready else {
            errors.push(issue(
                Cycle,
                None,
                None,
                format!(
                    "Remove cyclic data bindings among: {}.",
                    remaining.keys().copied().collect::<Vec<_>>().join(", ")
                ),
            ));
            break;
        };
        remaining.remove(ready);
        for deps in remaining.values_mut() {
            deps.remove(ready);
        }
        order.push(ready.to_owned());
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let has_v3 = contracts.iter().any(|c| c.contract.v3().is_some());
    let mut source_warmup: BTreeMap<String, u64> = BTreeMap::new();
    let mut history_by_output: BTreeMap<String, BTreeMap<String, BTreeMap<String, u64>>> =
        BTreeMap::new();
    for id in &order {
        let contract = &metadata[id.as_str()].contract;
        let own = warmups[id.as_str()];
        let resolved = contract
            .v3()
            .map(|v| v.resolve(&nodes[id.as_str()].parameters))
            .transpose()
            .map_err(|e| vec![issue(InvalidContract, Some(id), None, e)])?;
        let mut output_histories = BTreeMap::new();
        for output in contract.outputs.keys() {
            let mut roots: BTreeMap<String, u64> = BTreeMap::new();
            for (input, declaration) in contract.inputs.iter().filter(|(_, p)| p.required) {
                let _ = declaration;
                let binding = request
                    .bindings
                    .iter()
                    .find(|b| b.consumer_node == *id && b.input_port == *input)
                    .expect("required binding was validated");
                let count = resolved.as_ref().map_or(own, |r| r[output][input]);
                let upstream = &history_by_output[&binding.producer_node][&binding.output_port];
                for (source, history) in upstream {
                    let total=history.checked_add(count.saturating_sub(1)).filter(|n|*n<=if has_v3 {tradeassembly_plugin_sdk::data_history::MAX_HISTORY_OBSERVATIONS}else{MAX_EXACT_INTEGER})
                        .ok_or_else(||vec![issue(WarmupOverflow,Some(id),Some(output),format!("Input {input} history exceeds the supported observation budget."))])?;
                    roots
                        .entry(source.clone())
                        .and_modify(|n| *n = (*n).max(total))
                        .or_insert(total);
                }
            }
            if roots.is_empty() {
                roots.insert(id.clone(), own);
            }
            output_histories.insert(output.clone(), roots);
        }
        let total = output_histories
            .values()
            .flat_map(|r| r.values())
            .copied()
            .max()
            .unwrap_or(own);
        if has_v3 && total > tradeassembly_plugin_sdk::data_history::MAX_HISTORY_OBSERVATIONS {
            return Err(vec![issue(
                WarmupOverflow,
                Some(id),
                None,
                "Source history exceeds the supported observation budget.",
            )]);
        }
        source_warmup.insert(id.clone(), total);
        history_by_output.insert(id.clone(), output_histories);
    }
    let mut selected_source_history = BTreeMap::<String, u64>::new();
    for selected in &request.selected_outputs {
        for (source, count) in &history_by_output[&selected.node][&selected.port] {
            selected_source_history
                .entry(source.clone())
                .and_modify(|value| *value = (*value).max(*count))
                .or_insert(*count);
        }
    }
    let version = if !request.selected_outputs.is_empty() {
        DataPlanVersion::V5
    } else if !request.source_captures.is_empty() {
        DataPlanVersion::V4
    } else if has_v3 {
        DataPlanVersion::V3
    } else if contracts
        .iter()
        .any(|c| matches!(c.contract.schema_version, DataContractVersion::V2))
    {
        DataPlanVersion::V2
    } else {
        DataPlanVersion::V1
    };
    let mut plan = CompiledDataPlan {
        schema_version: version,
        capability_graph_fingerprint: graph.graph_fingerprint.clone(),
        evaluation_epoch: graph.evaluation_epoch.clone(),
        request,
        contracts,
        execution_order: order,
        source_warmup_observations: source_warmup,
        source_history_by_output: if has_v3 {
            history_by_output
        } else {
            BTreeMap::new()
        },
        selected_source_history,
        plan_sha256: String::new(),
    };
    plan.plan_sha256 = canonical_sha256(&plan).map_err(|_| {
        vec![issue(
            InvalidContract,
            None,
            None,
            "Plan could not be canonically encoded.",
        )]
    })?;
    Ok(plan)
}
