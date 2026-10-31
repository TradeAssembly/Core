//! Host materialization of verified declarative recipes into the existing DAG.
//! No calculation, credential grant, subprocess, provider substitution or order.
use super::{capability_graph, data_plans, TradeAssemblyService};
use crate::{data_pipeline::*, ports::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tradeassembly_plugin_sdk::{
    data_contract::DataPort, data_package::VersionedPackageDescriptor, data_recipe::*,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecipeProgramVersion {
    #[serde(rename = "tradeassembly.recipe_plan.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeProgram {
    pub schema_version: RecipeProgramVersion,
    pub base_request: PipelineRequest,
    pub instances: Vec<RecipeInvocation>,
    pub outputs: Vec<RecipeInput>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeInvocation {
    pub instance_id: String,
    pub plugin_instance_ref: String,
    pub package_sha256: String,
    pub recipe_id: String,
    pub semantics_version: String,
    pub parameters: Value,
    pub nodes: BTreeMap<String, RecipeNodeBinding>,
    pub inputs: BTreeMap<String, RecipeInput>,
    pub selected_outputs: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeNodeBinding {
    pub graph_node_id: String,
    pub context: DataContext,
    #[serde(default)]
    pub input_contexts: BTreeMap<String, DataContext>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RecipeInput {
    Pipeline { node: String, port: String },
    Recipe { instance: String, output: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipePin {
    pub instance_id: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub recipe_document_sha256: String,
    pub recipe_canonical_sha256: String,
    pub instance_context_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipePlanSource {
    pub resolved_outputs: Vec<ResolvedRecipeOutput>,
    pub program: RecipeProgram,
    pub pins: Vec<RecipePin>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedRecipeOutput {
    pub source: RecipeInput,
    pub output: PipelineOutput,
    pub data: DataPort,
    pub context: DataContext,
}
struct LoadedRecipe {
    definition: DataRecipe,
    plugin_ref: String,
    pin: RecipePin,
}
struct InstanceWork {
    invocation: RecipeInvocation,
    definition: DataRecipe,
    plugin_ref: String,
    inputs: BTreeMap<String, PipelineOutput>,
    active: BTreeSet<String>,
    emitted: BTreeMap<String, String>,
}
struct Expansion<'a> {
    service: &'a TradeAssemblyService,
    graph: CapabilityGraphResolution,
    pipeline: PipelineRequest,
    base_nodes: BTreeSet<String>,
    invocations: BTreeMap<String, RecipeInvocation>,
    loaded: BTreeMap<String, LoadedRecipe>,
    active: BTreeSet<String>,
    done: BTreeSet<String>,
    outputs: BTreeMap<(String, String), PipelineOutput>,
    expected: Vec<(PipelineOutput, DataPort)>,
}
fn code<T: Serialize>(value: &T) -> Result<String, String> {
    canonical_sha256(value).map_err(|_| "data_recipe_identity_invalid".into())
}
fn load(
    service: &TradeAssemblyService,
    invocation: &mut RecipeInvocation,
) -> Result<LoadedRecipe, String> {
    service
        .require_object("plugin_instance", &invocation.plugin_instance_ref)
        .map_err(|_| "data_recipe_not_available")?;
    let runtime = service.runtime();
    let instance = runtime
        .plugins
        .get_instance(&invocation.plugin_instance_ref)?
        .ok_or("data_recipe_instance_unavailable")?;
    if instance["instanceRef"] != invocation.plugin_instance_ref
        || instance["enabled"] != true
        || instance.get("removedAtMs").is_some_and(|v| !v.is_null())
        || instance["activePackageSha256"] != invocation.package_sha256
    {
        return Err("data_recipe_instance_unavailable".into());
    }
    let package = runtime
        .plugin_packages
        .get(&invocation.package_sha256)?
        .ok_or("data_recipe_package_missing")?;
    if package.package_sha256 != invocation.package_sha256
        || instance["pluginRef"] != package.plugin_ref
    {
        return Err("data_recipe_package_binding_mismatch".into());
    }
    let manifest = runtime
        .plugins
        .get_manifest(&package.plugin_ref)?
        .ok_or("data_recipe_manifest_missing")?;
    if manifest["manifestDigest"] != package.manifest_sha256
        || manifest["package"]["packageSha256"] != invocation.package_sha256
    {
        return Err("data_recipe_package_binding_mismatch".into());
    }
    let definition = runtime
        .plugin_packages
        .data_recipe(&invocation.package_sha256, &invocation.recipe_id)?
        .ok_or("data_recipe_missing")?;
    if definition.semantics_version != invocation.semantics_version {
        return Err("data_recipe_semantics_mismatch".into());
    }
    let parameters = invocation
        .parameters
        .as_object_mut()
        .ok_or("data_recipe_parameters_invalid")?;
    for (key, value) in &definition.parameter_defaults {
        parameters
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    tradeassembly_plugin_sdk::canonical::normalize_numbers(&mut invocation.parameters)
        .map_err(|_| "data_recipe_numeric_range_invalid")?;
    if !jsonschema::validator_for(&definition.parameter_schema)
        .map_err(|_| "data_recipe_parameter_schema_invalid")?
        .is_valid(&invocation.parameters)
    {
        return Err("data_recipe_parameters_invalid".into());
    }
    invocation.selected_outputs.sort();
    if invocation.selected_outputs.is_empty()
        || invocation.selected_outputs.len() > MAX_RECIPE_PORTS
        || invocation.selected_outputs.windows(2).any(|v| v[0] == v[1])
        || invocation
            .selected_outputs
            .iter()
            .any(|name| !definition.outputs.contains_key(name))
    {
        return Err("data_recipe_output_invalid".into());
    }
    if invocation
        .nodes
        .keys()
        .any(|name| !definition.nodes.contains_key(name))
        || invocation.inputs.keys().ne(definition.inputs.keys())
    {
        return Err("data_recipe_bindings_invalid".into());
    }
    let descriptor = VersionedPackageDescriptor::decode(
        &serde_json::to_vec(&package.descriptor).map_err(|_| "data_recipe_package_invalid")?,
    )?;
    let view = descriptor.data().ok_or("data_recipe_package_invalid")?;
    let entry = view
        .recipes
        .iter()
        .find(|r| r.recipe_id == invocation.recipe_id)
        .ok_or("data_recipe_missing")?;
    let pin = RecipePin {
        instance_id: invocation.instance_id.clone(),
        package_sha256: package.package_sha256,
        manifest_sha256: package.manifest_sha256,
        recipe_document_sha256: entry.document.sha256.clone(),
        recipe_canonical_sha256: entry.canonical_sha256.clone(),
        instance_context_sha256: code(
            &json!({"instance":invocation.plugin_instance_ref,"configuration":instance["configuration"],"accountRef":instance["accountRef"],"credentialRef":instance["credentialRef"],"credentialRevision":crate::live_execution_checks::credential_revision(&instance)}),
        )?,
    };
    Ok(LoadedRecipe {
        definition,
        plugin_ref: package.plugin_ref,
        pin,
    })
}
impl Expansion<'_> {
    fn source(&mut self, source: &RecipeInput) -> Result<PipelineOutput, String> {
        match source {
            RecipeInput::Pipeline { node, port } => {
                if !self.base_nodes.contains(node) {
                    return Err("data_recipe_input_not_base_node".into());
                }
                Ok(PipelineOutput {
                    node: node.clone(),
                    port: port.clone(),
                })
            }
            RecipeInput::Recipe { instance, output } => {
                self.instance(instance)?;
                self.outputs
                    .get(&(instance.clone(), output.clone()))
                    .cloned()
                    .ok_or("data_recipe_dependency_output_missing".into())
            }
        }
    }
    fn instance(&mut self, id: &str) -> Result<(), String> {
        if self.done.contains(id) {
            return Ok(());
        }
        if !self.active.insert(id.into()) {
            return Err("data_recipe_instance_cycle".into());
        }
        let invocation = self
            .invocations
            .get(id)
            .cloned()
            .ok_or("data_recipe_instance_missing")?;
        let loaded = self.loaded.get(id).ok_or("data_recipe_instance_missing")?;
        let mut work = InstanceWork {
            invocation: invocation.clone(),
            definition: loaded.definition.clone(),
            plugin_ref: loaded.plugin_ref.clone(),
            inputs: BTreeMap::new(),
            active: BTreeSet::new(),
            emitted: BTreeMap::new(),
        };
        for (name, source) in &invocation.inputs {
            let producer = self.source(source)?;
            self.expected
                .push((producer.clone(), work.definition.inputs[name].clone()));
            work.inputs.insert(name.clone(), producer);
        }
        for name in &invocation.selected_outputs {
            let output = work.definition.outputs[name].clone();
            let producer = self.port(&mut work, &output.producer)?;
            self.expected.push((producer.clone(), output.data));
            self.outputs.insert((id.into(), name.clone()), producer);
        }
        self.active.remove(id);
        self.done.insert(id.into());
        Ok(())
    }
    fn port(
        &mut self,
        work: &mut InstanceWork,
        port: &RecipePort,
    ) -> Result<PipelineOutput, String> {
        match work
            .definition
            .resolve_port(port, &work.invocation.parameters)?
        {
            RecipePort::Input { port } => work
                .inputs
                .get(&port)
                .cloned()
                .ok_or("data_recipe_input_missing".into()),
            RecipePort::Output { node, port } => {
                let node = self.node(work, &node)?;
                Ok(PipelineOutput { node, port })
            }
            RecipePort::Choose { .. } => Err("data_recipe_choice_unresolved".into()),
        }
    }
    fn node(&mut self, work: &mut InstanceWork, id: &str) -> Result<String, String> {
        if let Some(node) = work.emitted.get(id) {
            return Ok(node.clone());
        }
        if !work.active.insert(id.into()) {
            return Err("data_recipe_cycle".into());
        }
        let definition = work
            .definition
            .nodes
            .get(id)
            .cloned()
            .ok_or("data_recipe_node_missing")?;
        let binding = work
            .invocation
            .nodes
            .get(id)
            .cloned()
            .ok_or("data_recipe_node_binding_missing")?;
        if self
            .pipeline
            .nodes
            .iter()
            .any(|node| node.graph_node_id == binding.graph_node_id)
        {
            return Err("data_recipe_node_binding_conflict".into());
        }
        let selected = self
            .graph
            .nodes
            .iter()
            .find(|node| node.node_id == binding.graph_node_id)
            .and_then(|node| node.selected.as_ref())
            .ok_or("data_recipe_node_unresolved")?;
        let expected_plugin = if definition.operation.plugin_ref == "self" {
            &work.plugin_ref
        } else {
            &definition.operation.plugin_ref
        };
        if selected.plugin_ref != *expected_plugin
            || selected.operation_id != definition.operation.operation_id
        {
            return Err("data_recipe_operation_binding_mismatch".into());
        }
        let pin = super::data_binding::pin(self.service, selected)?;
        let contract = self
            .service
            .runtime()
            .plugin_packages
            .operation_data(&pin.package_sha256, &selected.operation_id)?
            .ok_or("data_recipe_contract_missing")?;
        if contract.semantics_version != definition.operation.semantics_version {
            return Err("data_recipe_semantics_mismatch".into());
        }
        let mut parameters = serde_json::Map::new();
        for (name, value) in definition.parameters {
            parameters.insert(
                name,
                match value {
                    RecipeParameter::Constant { value } => value,
                    RecipeParameter::Parameter { name } => work
                        .invocation
                        .parameters
                        .get(&name)
                        .cloned()
                        .ok_or("data_recipe_parameter_missing")?,
                },
            );
        }
        let incoming = work
            .definition
            .bindings
            .iter()
            .filter(|edge| edge.consumer_node == id)
            .cloned()
            .collect::<Vec<_>>();
        for edge in incoming {
            let producer = self.port(work, &edge.producer)?;
            self.pipeline.bindings.push(PortBinding {
                producer_node: producer.node,
                output_port: producer.port,
                consumer_node: binding.graph_node_id.clone(),
                input_port: edge.input_port,
            });
        }
        if self.pipeline.nodes.len() >= 32 {
            return Err("data_recipe_expansion_bounds_invalid".into());
        }
        self.pipeline.nodes.push(PipelineNode {
            graph_node_id: binding.graph_node_id.clone(),
            parameters: Value::Object(parameters),
            context: binding.context,
            input_contexts: binding.input_contexts,
        });
        work.active.remove(id);
        work.emitted
            .insert(id.into(), binding.graph_node_id.clone());
        Ok(binding.graph_node_id)
    }
}

pub(crate) fn prepare(
    service: &TradeAssemblyService,
    scope: data_plans::PlanScope,
    mut program: RecipeProgram,
) -> Result<data_plans::DataPlanContent, String> {
    service
        .require_object("strategy", &scope.strategy_id)
        .map_err(|_| "data_recipe_not_available")?;
    let bytes = serde_json::to_vec(&program).map_err(|_| "data_recipe_program_invalid")?;
    if bytes.len() > tradeassembly_plugin_sdk::data_package::MAX_METADATA_DOCUMENT_BYTES
        || program.instances.is_empty()
        || program.instances.len() > 32
        || program.outputs.is_empty()
        || program.outputs.len() > 256
    {
        return Err("data_recipe_program_bounds_invalid".into());
    }
    tradeassembly_plugin_sdk::data_package::metadata_budget(
        &serde_json::to_value(&program).map_err(|_| "data_recipe_program_invalid")?,
    )?;
    if !normalize_request(&mut program.base_request).is_empty() {
        return Err("data_recipe_base_request_invalid".into());
    }
    program
        .instances
        .sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
    let mut invocations = BTreeMap::new();
    let mut loaded = BTreeMap::new();
    for invocation in &mut program.instances {
        if invocation.instance_id.is_empty()
            || invocation.instance_id.len() > 128
            || invocations.contains_key(&invocation.instance_id)
        {
            return Err("data_recipe_instance_identity_invalid".into());
        }
        let recipe = load(service, invocation)?;
        loaded.insert(invocation.instance_id.clone(), recipe);
        invocations.insert(invocation.instance_id.clone(), invocation.clone());
    }
    let revision =
        capability_graph::get_revision(service, &program.base_request.capability_graph_revision);
    let graph: CapabilityGraphResolution =
        serde_json::from_value(revision["revision"]["graph"].clone())
            .map_err(|_| "data_recipe_graph_unavailable")?;
    let base_nodes = program
        .base_request
        .nodes
        .iter()
        .map(|node| node.graph_node_id.clone())
        .collect();
    let mut expansion = Expansion {
        service,
        graph,
        pipeline: program.base_request.clone(),
        base_nodes,
        invocations,
        loaded,
        active: BTreeSet::new(),
        done: BTreeSet::new(),
        outputs: BTreeMap::new(),
        expected: vec![],
    };
    let mut selected = BTreeSet::new();
    let mut logical = BTreeSet::new();
    let mut resolved = Vec::new();
    for output in &program.outputs {
        if !logical.insert(code(output)?) {
            return Err("data_recipe_output_duplicate".into());
        }
        let port = expansion.source(output)?;
        selected.insert(port.clone());
        resolved.push((output.clone(), port));
    }
    if expansion.done.len() != program.instances.len() {
        return Err("data_recipe_instance_unused".into());
    }
    expansion.pipeline.selected_outputs = selected.into_iter().collect();
    let mut content = data_plans::prepare(service, scope, expansion.pipeline)?;
    for (output, expected) in expansion.expected {
        let actual = content
            .plan
            .contracts
            .iter()
            .find(|contract| contract.graph_node_id == output.node)
            .and_then(|contract| contract.contract.outputs.get(&output.port))
            .ok_or("data_recipe_output_port_missing")?;
        if actual != &expected {
            return Err("data_recipe_port_contract_mismatch".into());
        }
    }
    let mut resolved_outputs = Vec::new();
    for (source, output) in resolved {
        let data = content
            .plan
            .contracts
            .iter()
            .find(|contract| contract.graph_node_id == output.node)
            .and_then(|contract| contract.contract.outputs.get(&output.port))
            .cloned()
            .ok_or("data_recipe_output_port_missing")?;
        let context = content
            .plan
            .request
            .nodes
            .iter()
            .find(|node| node.graph_node_id == output.node)
            .map(|node| node.context.clone())
            .ok_or("data_recipe_output_port_missing")?;
        resolved_outputs.push(ResolvedRecipeOutput {
            source,
            output,
            data,
            context,
        });
    }
    content.recipe_source = Some(RecipePlanSource {
        resolved_outputs,
        program,
        pins: expansion
            .loaded
            .into_values()
            .map(|loaded| loaded.pin)
            .collect(),
    });
    Ok(content)
}
pub fn save(
    service: &TradeAssemblyService,
    scope: data_plans::PlanScope,
    program: RecipeProgram,
    context: &SideEffectContext,
) -> Result<data_plans::StoredDataPlan, String> {
    let content = prepare(service, scope, program)?;
    data_plans::persist(service.runtime().storage.as_ref(), content, context)
}

const PREPARE_PATH: &str = "/product/strategies/data-recipes/prepare";
fn body_program(body: &Value) -> Result<(data_plans::PlanScope, RecipeProgram), String> {
    let scope =
        serde_json::from_value(body["scope"].clone()).map_err(|_| "data_recipe_scope_invalid")?;
    let raw = &body["program"];
    if serde_json::to_vec(raw)
        .map_err(|_| "data_recipe_program_invalid")?
        .len()
        > tradeassembly_plugin_sdk::data_package::MAX_METADATA_DOCUMENT_BYTES
    {
        return Err("data_recipe_program_bounds_invalid".into());
    }
    tradeassembly_plugin_sdk::data_package::metadata_budget(raw)?;
    let program = serde_json::from_value(raw.clone()).map_err(|_| "data_recipe_program_invalid")?;
    Ok((scope, program))
}
fn error_response(code: &str) -> super::ServiceResponse {
    if let Some(encoded) = code.strip_prefix("data_plan_compile_failed:") {
        let diagnostics = serde_json::from_str::<Vec<PlanDiagnostic>>(encoded).unwrap_or_default();
        let mut response = super::ServiceResponse::conflict("data_recipe_plan_compile_failed");
        response.body["diagnostics"] = json!(diagnostics);
        response.body["error"]["details"] = json!({"diagnostics":diagnostics});
        return response;
    }

    if matches!(
        code,
        "data_recipe_scope_invalid"
            | "data_recipe_step_invalid"
            | "data_recipe_program_invalid"
            | "data_recipe_idempotency_required"
            | "data_recipe_program_bounds_invalid"
    ) {
        super::ServiceResponse::bad_request(code)
    } else {
        super::ServiceResponse::conflict(code)
    }
}
pub fn prepare_response(service: &TradeAssemblyService, body: Value) -> super::ServiceResponse {
    let result: Result<data_plans::StoredDataPlan, String> = (|| {
        let (scope, program) = body_program(&body)?;
        let key = body["idempotencyKey"]
            .as_str()
            .ok_or("data_recipe_idempotency_required")?;
        let authority =
            crate::control_plane::http_envelope("http", "POST", PREPARE_PATH, &body)?.authority;
        let context = SideEffectContext::new(
            authority,
            IdempotencyKey::new(key).map_err(|_| "data_recipe_idempotency_required")?,
        )
        .with_agent_execution(service.agent_mcp_execution_context());
        save(service, scope, program, &context)
    })();
    match result {
        Ok(plan) => {
            super::ServiceResponse::ok(json!({"plan":plan,"executionAuthorityGranted":false}))
        }
        Err(code) => error_response(&code),
    }
}
pub(crate) fn preflight(
    service: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<super::ServiceResponse> {
    let result = match path {
        PREPARE_PATH => (|| {
            let (scope, program) = body_program(body)?;
            prepare(service, scope, program).map(|_| ())
        })(),
        "/product/strategies/data-recipes/step" => step_request(service, body).map(|_| ()),
        _ => return None,
    };
    result.err().map(|code: String| error_response(&code))
}

fn step_request(
    service: &TradeAssemblyService,
    body: &Value,
) -> Result<super::data_evaluation::EvaluationStep, String> {
    let request: super::data_evaluation::EvaluationStep =
        serde_json::from_value(body["step"].clone()).map_err(|_| "data_recipe_step_invalid")?;
    let plan = super::data_evaluation::request_plan(service, &request)?;
    if plan.content.recipe_source.is_none() {
        return Err("data_recipe_plan_required".into());
    }
    super::data_evaluation::remember_request_plan(service, &request, &plan)?;
    Ok(request)
}
pub fn step_response(service: &TradeAssemblyService, body: Value) -> super::ServiceResponse {
    let result: Result<super::data_evaluation::EvaluationReceipt, String> = (|| {
        let request = step_request(service, &body)?;
        let key = body["idempotencyKey"]
            .as_str()
            .ok_or("data_recipe_idempotency_required")?;
        let authority = crate::control_plane::http_envelope(
            "http",
            "POST",
            "/product/strategies/data-recipes/step",
            &body,
        )?
        .authority;
        let context = SideEffectContext::new(
            authority,
            IdempotencyKey::new(key).map_err(|_| "data_recipe_idempotency_required")?,
        )
        .with_agent_execution(service.agent_mcp_execution_context());
        super::data_evaluation::step(service, request, &context)
    })();
    match result {
        Ok(receipt) => {
            super::ServiceResponse::ok(json!({"receipt":receipt,"executionAuthorityGranted":false}))
        }
        Err(code) => error_response(&code),
    }
}
