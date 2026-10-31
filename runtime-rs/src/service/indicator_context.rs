//! Read-only, publisher-bound contextual evidence. Never invocation authority.
//! No provider/HTTP child, plan preparation, credential resolution or persistent write.
use super::{
    capability_graph, data_captures, data_evaluation, data_plans, data_recipes, ServiceResponse,
    TradeAssemblyService,
};
use crate::data_pipeline::{InstrumentContext, PipelineOutput};
use crate::ports::{EntitlementGrant, InstalledPluginPackage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tradeassembly_plugin_sdk::{
    data_evaluation::{DataPoint, PointValue},
    data_package::VersionedPackageDescriptor,
    data_recipe::DataRecipe,
};

pub(crate) const PATH: &str = "/product/strategies/indicator-context/inspect";
pub(crate) const TOOL: &str = "tradeassembly.strategy.indicator_context.inspect";
const MAX_REPORT: usize = 262_144;
const MAX_EXACT: i64 = tradeassembly_plugin_sdk::canonical::MAX_EXACT_INTEGER as i64;
pub(crate) fn binding_schema() -> Value {
    serde_json::from_str(include_str!("indicator_context/binding.schema.json"))
        .expect("fixed binding schema")
}
pub(crate) fn request_schema() -> Value {
    serde_json::from_str(include_str!("indicator_context/request.schema.json"))
        .expect("fixed request schema")
}
pub(crate) fn response_schema() -> Value {
    serde_json::from_str(include_str!("indicator_context/response.schema.json"))
        .expect("fixed response schema")
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Catalog {
    schema_version: String,
    byte_sha256: String,
    canonical_sha256: String,
    canonicalization: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecipeIdentity {
    recipe_id: String,
    semantics_version: String,
    canonical_sha256: String,
    document_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SchemaIdentity {
    schema_ref: String,
    canonical_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Gate {
    HostCapture {
        input: String,
        acquisition_schema: SchemaIdentity,
        origin_pointer: String,
    },
    CaptureAnchor {
        input: String,
        anchor_pointer: String,
        boundary: String,
    },
    StrictFixedIntervalHistory {},
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Estimate {
    Never {},
    Always {},
    OutputBoolean { output: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Output {
    name: String,
    implementation_status: String,
    schema: SchemaIdentity,
    origin: String,
    model_id: String,
    model_version: String,
    units: String,
    estimate: Estimate,
    gates: Vec<Gate>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Profile {
    concept_id: String,
    profile_id: String,
    profile_version: String,
    recipe: Option<RecipeIdentity>,
    outputs: Vec<Output>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    schema_version: String,
    catalog: Catalog,
    profiles: Vec<Profile>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    plugin_instance_ref: String,
    package_sha256: String,
    catalog: Catalog,
    concept_id: String,
    profile_id: String,
    profile_version: String,
    recipe: Option<RecipeIdentity>,
    selected_outputs: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Evaluation {
    receipt_sha256: String,
    step: data_evaluation::EvaluationStep,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    schema_version: String,
    scope: data_plans::PlanScope,
    plan_revision_id: String,
    recipe_instance_id: Option<String>,
    selection: Selection,
    subject_instrument_id: String,
    evaluation: Option<Evaluation>,
    allow_estimates: bool,
}
fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    crate::data_pipeline::canonical_sha256(value)
        .map_err(|_| "indicator_context_encoding_invalid".into())
}
fn validate(value: &Value, schema: &Value, limit: usize) -> Result<(), String> {
    if serde_json::to_vec(value)
        .map_err(|_| "indicator_context_encoding_invalid")?
        .len()
        > limit
    {
        return Err("indicator_context_bounds_exceeded".into());
    }
    tradeassembly_plugin_sdk::data_package::metadata_budget(value)?;
    if !jsonschema::validator_for(schema)
        .map_err(|_| "indicator_context_schema_invalid")?
        .is_valid(value)
    {
        return Err("indicator_context_contract_invalid".into());
    }
    Ok(())
}
fn owner(s: &TradeAssemblyService) -> Result<(String, i64), String> {
    let now = s.runtime().clock.trusted_now_ms()?;
    let (issuer, subject) = s
        .responsible_human_identity()
        .ok_or("indicator_context_owner_required")?;
    let identity = s
        .runtime()
        .identity
        .resolve(subject, now)
        .map_err(|_| "indicator_context_identity_unavailable")?;
    if identity.issuer != issuer || identity.expires_at_ms <= now {
        return Err("indicator_context_identity_stale".into());
    }
    Ok((
        hash(
            &s.invocation_owner()
                .ok_or("indicator_context_owner_required")?,
        )?,
        identity.expires_at_ms,
    ))
}
fn subject(
    plan: &data_plans::StoredDataPlan,
    id: &str,
) -> Result<(InstrumentContext, Option<u64>), String> {
    let mut matches = Vec::new();
    for node in &plan.content.plan.request.nodes {
        for context in std::iter::once(&node.context).chain(node.input_contexts.values()) {
            for instrument in &context.instruments {
                if instrument.instrument_id == id {
                    matches.push((instrument.clone(), context.interval_ms));
                }
            }
        }
    }
    let first = matches
        .first()
        .cloned()
        .ok_or("indicator_context_subject_not_bound")?;
    if matches.iter().any(|m| m.0 != first.0) {
        return Err("indicator_context_subject_context_ambiguous".into());
    }
    if matches.iter().any(|m| m.1 != first.1) {
        return Ok((first.0, None));
    }
    Ok(first)
}
struct OutputReport {
    raw: Value,
}
impl OutputReport {
    fn new(name: &str) -> Self {
        Self {
            raw: json!({"name":name,"availability":"not_checked","origin":"not_checked","modelId":null,"modelVersion":null,"units":null,"usesEstimatedInputs":null,"schema":null,"temporal":null,"checks":[],"blockers":[]}),
        }
    }
    fn check(
        &mut self,
        check: &str,
        state: &str,
        code: &str,
        availability: Option<&str>,
        refs: Vec<String>,
        node: Option<&str>,
    ) {
        self.raw["checks"].as_array_mut().expect("checks").push(
            json!({"check":check,"state":state,"code":code,"evidenceRefs":refs,"nodeId":node}),
        );
        if let Some(availability) = availability {
            self.raw["blockers"].as_array_mut().expect("blockers").push(
                json!({"availability":availability,"check":check,"code":code,"evidenceRefs":refs}),
            );
        }
    }
    fn satisfied(&mut self, check: &str, code: &str, refs: Vec<String>) {
        self.check(check, "satisfied", code, None, refs, None);
    }
    fn blocked(&mut self, check: &str, code: &str, status: &str) {
        self.check(check, "blocked", code, Some(status), Vec::new(), None);
    }
    fn unknown(&mut self, check: &str, code: &str) {
        self.check(
            check,
            "not_checked",
            code,
            Some("not_checked"),
            Vec::new(),
            None,
        );
    }
    fn finish(mut self) -> Value {
        let blockers = self.raw["blockers"].as_array().expect("blockers");
        let primary = if blockers.is_empty() {
            "available"
        } else if blockers.iter().any(|b| b["availability"] == "planned_only") {
            "planned_only"
        } else if blockers.iter().any(|b| b["availability"] == "not_checked") {
            "not_checked"
        } else {
            blockers[0]["availability"]
                .as_str()
                .unwrap_or("not_checked")
        };
        self.raw["availability"] = json!(primary);
        self.raw
    }
}
struct Published {
    package: InstalledPluginPackage,
    binding: Binding,
    profile: Profile,
    binding_hash: String,
    recipe: Option<DataRecipe>,
}
fn published(s: &TradeAssemblyService, r: &Request) -> Result<Published, String> {
    s.require_object("plugin_instance", &r.selection.plugin_instance_ref)
        .map_err(|_| "indicator_context_not_available")?;
    let instance = s
        .runtime()
        .plugins
        .get_instance(&r.selection.plugin_instance_ref)?
        .ok_or("indicator_context_instance_missing")?;
    if instance["enabled"] != true
        || instance.get("removedAtMs").is_some_and(|v| !v.is_null())
        || instance["activePackageSha256"] != r.selection.package_sha256
    {
        return Err("indicator_context_installation_stale".into());
    }
    let package = s
        .runtime()
        .plugin_packages
        .get(&r.selection.package_sha256)?
        .ok_or("indicator_context_package_missing")?;
    let manifest = s
        .runtime()
        .plugins
        .get_manifest(&package.plugin_ref)?
        .ok_or("indicator_context_manifest_missing")?;
    if package.package_sha256 != r.selection.package_sha256
        || instance["pluginRef"] != package.plugin_ref
        || manifest["manifestDigest"] != package.manifest_sha256
        || manifest["package"]["packageSha256"] != package.package_sha256
        || manifest["manifest"] != package.manifest
    {
        return Err("indicator_context_installation_stale".into());
    }
    let raw = &package.manifest["metadata"]["indicatorProfileBindings"];
    if raw.is_null() {
        return Err("catalog_not_admitted".into());
    }
    validate(raw, &binding_schema(), 262_144)
        .map_err(|_| "indicator_context_binding_contract_unsupported")?;
    let binding: Binding = serde_json::from_value(raw.clone())
        .map_err(|_| "indicator_context_binding_contract_unsupported")?;
    let mut identities = BTreeSet::new();
    for p in &binding.profiles {
        if !identities.insert((&p.concept_id, &p.profile_id, &p.profile_version))
            || p.outputs
                .iter()
                .map(|o| &o.name)
                .collect::<BTreeSet<_>>()
                .len()
                != p.outputs.len()
        {
            return Err("indicator_context_binding_ambiguous".into());
        }
    }
    let matched = binding
        .profiles
        .iter()
        .filter(|p| {
            p.profile_id == r.selection.profile_id
                && p.profile_version == r.selection.profile_version
                && p.concept_id == r.selection.concept_id
        })
        .collect::<Vec<_>>();
    if matched.len() != 1 || binding.catalog != r.selection.catalog {
        return Err("indicator_context_profile_not_admitted".into());
    }
    let profile = matched[0].clone();
    if profile.recipe != r.selection.recipe
        || r.selection
            .selected_outputs
            .iter()
            .any(|out| !profile.outputs.iter().any(|o| &o.name == out))
    {
        return Err("indicator_context_profile_binding_mismatch".into());
    }
    let recipe = if let Some(expected) = &profile.recipe {
        let recipe = s
            .runtime()
            .plugin_packages
            .data_recipe(&package.package_sha256, &expected.recipe_id)?
            .ok_or("indicator_context_recipe_missing")?;
        let descriptor = VersionedPackageDescriptor::decode(
            &serde_json::to_vec(&package.descriptor)
                .map_err(|_| "indicator_context_encoding_invalid")?,
        )?;
        let data = descriptor
            .data()
            .ok_or("indicator_context_recipe_missing")?;
        let entry = data
            .recipes
            .iter()
            .find(|p| p.recipe_id == expected.recipe_id)
            .ok_or("indicator_context_recipe_missing")?;
        if recipe.semantics_version != expected.semantics_version
            || hash(&recipe)? != expected.canonical_sha256
            || entry.canonical_sha256 != expected.canonical_sha256
            || entry.document.sha256 != expected.document_sha256
        {
            return Err("indicator_context_recipe_binding_mismatch".into());
        }
        for output in profile
            .outputs
            .iter()
            .filter(|o| o.implementation_status == "implemented")
        {
            let actual = recipe
                .outputs
                .get(&output.name)
                .ok_or("indicator_context_output_missing")?;
            if actual.data.schema.schema_ref != output.schema.schema_ref
                || actual.data.schema.sha256 != output.schema.canonical_sha256
            {
                return Err("indicator_context_output_schema_mismatch".into());
            }
        }
        Some(recipe)
    } else {
        None
    };
    Ok(Published {
        binding_hash: hash(raw)?,
        package,
        binding,
        profile,
        recipe,
    })
}
fn original_evaluation(
    s: &TradeAssemblyService,
    r: &Request,
    plan: &data_plans::StoredDataPlan,
) -> Result<data_evaluation::EvaluationReceipt, String> {
    let expected = r.evaluation.as_ref().ok_or("evaluation_not_supplied")?;
    if expected.step.plan_revision_id != plan.revision_id
        || expected.step.window.event_start_ms <= 0
        || expected.step.window.event_start_ms >= expected.step.window.event_end_ms
        || expected.step.window.event_end_ms > expected.step.window.as_of_ms
        || expected.step.window.as_of_ms > s.runtime().clock.trusted_now_ms()?
    {
        return Err("evaluation_window_invalid".into());
    }
    let (actual_plan, receipt) = data_evaluation::indexed_receipt(s, &expected.receipt_sha256)?;
    if actual_plan != *plan {
        return Err("evaluation_plan_changed".into());
    }
    let authority = receipt
        .request_authority
        .as_ref()
        .ok_or("evaluation_original_authority_missing")?;
    if receipt.request_sha256 != hash(&json!({"request":expected.step,"authority":authority}))?
        || receipt.checkpoint.run_id != expected.step.run_id
    {
        return Err("evaluation_original_request_mismatch".into());
    }
    data_captures::validate_window(s, plan, &expected.step.window)?;
    for availability in receipt.checkpoint.state_availability.values().chain(
        receipt
            .checkpoint
            .partition_availability
            .values()
            .flat_map(|v| v.values()),
    ) {
        if availability.known_at_ms <= 0
            || availability.arrived_at_ms < availability.known_at_ms
            || availability.arrived_at_ms > expected.step.window.as_of_ms
        {
            return Err("evaluation_state_clock_invalid".into());
        }
    }
    for node in receipt.checkpoint.states.keys() {
        if !receipt.checkpoint.state_availability.contains_key(node) {
            return Err("evaluation_state_clock_unproven".into());
        }
    }
    for entry in &plan.content.plan.contracts {
        let node = plan
            .content
            .plan
            .request
            .nodes
            .iter()
            .find(|n| n.graph_node_id == entry.graph_node_id)
            .ok_or("evaluation_node_missing")?;
        if let Some(responses) = receipt.outputs.get(&entry.graph_node_id) {
            for response in responses {
                for (port, batch) in &response.outputs {
                    let data = entry
                        .contract
                        .outputs
                        .get(port)
                        .ok_or("evaluation_output_undeclared")?;
                    data_evaluation::validate_batch(
                        s,
                        &plan.content.package_sha256[&entry.graph_node_id],
                        data,
                        &node.context,
                        batch,
                        &expected.step.window,
                    )?;
                }
            }
        }
    }
    let window = &expected.step.window;
    let mut known_hashes = BTreeSet::from([hash(&BTreeMap::<String, Value>::new())?]);
    for responses in receipt.outputs.values() {
        for response in responses {
            known_hashes.insert(hash(&response.state)?);
            for batch in response.outputs.values() {
                for point in &batch.points {
                    known_hashes.insert(point.provenance.content_sha256.clone());
                }
            }
        }
    }
    for (root, authentication) in &plan.content.source_authentication {
        if let data_captures::SourceAuthentication::HostAcquisition { .. } = authentication {
            let captured = capture(s, plan, root, r)?;
            known_hashes.insert(captured.receipt_sha256.clone());
            known_hashes.insert(captured.payload_sha256.clone());
            for batch in captured.replay_response.outputs.values() {
                for point in &batch.points {
                    known_hashes.insert(point.provenance.content_sha256.clone());
                }
            }
        }
    }
    for reset in receipt
        .checkpoint
        .partition_resets
        .values()
        .flat_map(|slots| slots.values())
    {
        known_hashes.insert(reset.clone());
    }
    for (node, response) in &receipt.checkpoint.last_frame {
        let entry = plan
            .content
            .plan
            .contracts
            .iter()
            .find(|c| &c.graph_node_id == node)
            .ok_or("evaluation_last_frame_undeclared")?;
        let context = &plan
            .content
            .plan
            .request
            .nodes
            .iter()
            .find(|n| &n.graph_node_id == node)
            .ok_or("evaluation_last_frame_undeclared")?
            .context;
        for (port, batch) in &response.outputs {
            data_evaluation::validate_batch(
                s,
                &plan.content.package_sha256[node],
                entry
                    .contract
                    .outputs
                    .get(port)
                    .ok_or("evaluation_last_frame_undeclared")?,
                context,
                batch,
                window,
            )?;
        }
    }
    for (node, responses) in &receipt.outputs {
        let contract = plan
            .content
            .plan
            .contracts
            .iter()
            .find(|c| &c.graph_node_id == node)
            .ok_or("evaluation_node_undeclared")?;
        for response in responses {
            for batch in response.outputs.values() {
                for point in &batch.points {
                    if !contract.contract.inputs.is_empty()
                        && matches!(point.result, PointValue::Ready(_))
                        && point.provenance.upstream_content_sha256.is_empty()
                    {
                        return Err("evaluation_lineage_unproven".into());
                    }
                    if point
                        .provenance
                        .upstream_content_sha256
                        .iter()
                        .any(|h| !known_hashes.contains(h))
                    {
                        return Err("evaluation_lineage_unproven".into());
                    }
                }
            }
        }
    }
    Ok(receipt)
}
fn resolved(
    plan: &data_plans::StoredDataPlan,
    instance: &str,
    output: &str,
) -> Option<PipelineOutput> {
    plan.content
        .recipe_source
        .as_ref()?
        .resolved_outputs
        .iter()
        .find_map(|r| match &r.source {
            data_recipes::RecipeInput::Recipe {
                instance: i,
                output: o,
            } if i == instance && o == output => Some(r.output.clone()),
            _ => None,
        })
}
fn input_output(
    plan: &data_plans::StoredDataPlan,
    source: &data_recipes::RecipeInput,
) -> Option<PipelineOutput> {
    match source {
        data_recipes::RecipeInput::Pipeline { node, port } => Some(PipelineOutput {
            node: node.clone(),
            port: port.clone(),
        }),
        data_recipes::RecipeInput::Recipe { instance, output } => resolved(plan, instance, output),
    }
}
fn roots(
    plan: &data_plans::StoredDataPlan,
    out: &PipelineOutput,
) -> Result<BTreeSet<String>, String> {
    let mut pending = vec![out.node.clone()];
    let mut visited = BTreeSet::new();
    let mut roots = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if !visited.insert(node.clone()) {
            continue;
        }
        if visited.len() > 32 {
            return Err("indicator_context_graph_bounds".into());
        }
        let contract = plan
            .content
            .plan
            .contracts
            .iter()
            .find(|c| c.graph_node_id == node)
            .ok_or("indicator_context_node_missing")?;
        if contract.contract.inputs.is_empty() {
            roots.insert(node);
            continue;
        }
        for (input, spec) in &contract.contract.inputs {
            if !spec.required {
                continue;
            }
            let edge = plan
                .content
                .plan
                .request
                .bindings
                .iter()
                .find(|b| b.consumer_node == node && &b.input_port == input)
                .ok_or("indicator_context_source_path_missing")?;
            pending.push(edge.producer_node.clone());
        }
    }
    Ok(roots)
}
fn last_point<'a>(
    receipt: &'a data_evaluation::EvaluationReceipt,
    out: &PipelineOutput,
    subject: &str,
    end: i64,
) -> Option<&'a DataPoint> {
    receipt
        .outputs
        .get(&out.node)?
        .iter()
        .flat_map(|r| {
            r.outputs
                .get(&out.port)
                .into_iter()
                .flat_map(|b| b.points.iter())
        })
        .find(|p| p.instrument_id == subject && p.provenance.event_end_ms == end)
}
fn recipe_matches(plan: &data_plans::StoredDataPlan, r: &Request, p: &Published) -> bool {
    let Some(instance) = r.recipe_instance_id.as_deref() else {
        return false;
    };
    let Some(source) = plan.content.recipe_source.as_ref() else {
        return false;
    };
    let Some(expected) = p.profile.recipe.as_ref() else {
        return false;
    };
    source.program.instances.iter().any(|i| {
        i.instance_id == instance
            && i.plugin_instance_ref == r.selection.plugin_instance_ref
            && i.package_sha256 == p.package.package_sha256
            && i.recipe_id == expected.recipe_id
            && i.semantics_version == expected.semantics_version
    }) && source.pins.iter().any(|pin| {
        pin.instance_id == instance
            && pin.package_sha256 == p.package.package_sha256
            && pin.manifest_sha256 == p.package.manifest_sha256
            && pin.recipe_document_sha256 == expected.document_sha256
            && pin.recipe_canonical_sha256 == expected.canonical_sha256
    })
}
fn input_roots(
    plan: &data_plans::StoredDataPlan,
    instance: &str,
    input: &str,
) -> Result<BTreeSet<String>, String> {
    let invocation = plan
        .content
        .recipe_source
        .as_ref()
        .and_then(|s| {
            s.program
                .instances
                .iter()
                .find(|i| i.instance_id == instance)
        })
        .ok_or("indicator_context_recipe_instance_missing")?;
    let source = invocation
        .inputs
        .get(input)
        .ok_or("indicator_context_recipe_input_missing")?;
    roots(
        plan,
        &input_output(plan, source).ok_or("indicator_context_recipe_input_unresolved")?,
    )
}
fn permission_snapshot(s: &TradeAssemblyService) -> Result<Value, String> {
    let rows = s.runtime().storage.list_json("entitlement_grants")?;
    let mut grants = Vec::new();
    for (_, raw) in rows {
        let _: EntitlementGrant =
            serde_json::from_value(raw.clone()).map_err(|_| "entitlement_snapshot_invalid")?;
        grants.push(raw);
    }
    grants.sort_by_key(|v| v["grantId"].as_str().unwrap_or_default().to_owned());
    Ok(json!(grants))
}
fn temporal(
    point: &DataPoint,
    window: &tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow,
) -> Result<Value, String> {
    Ok(
        json!({"eventStartMs":point.provenance.event_start_ms,"eventEndMs":point.provenance.event_end_ms,"asOfMs":window.as_of_ms,
        "latestKnownAtMs":point.provenance.known_at_ms,"latestArrivedAtMs":point.provenance.arrived_at_ms,"completed":point.provenance.completed,
        "sourceRevisionSha256":hash(&point.provenance.source_revision)?,"pointContentSha256":point.provenance.content_sha256}),
    )
}
fn capture(
    s: &TradeAssemblyService,
    plan: &data_plans::StoredDataPlan,
    root: &str,
    r: &Request,
) -> Result<data_captures::CaptureReceipt, String> {
    let Some(data_captures::SourceAuthentication::HostAcquisition {
        capture_id,
        receipt_sha256,
        payload_sha256,
    }) = plan.content.source_authentication.get(root)
    else {
        return Err("source_authentication_unverified".into());
    };
    let actual = data_captures::current(s, capture_id)?;
    if actual.receipt_sha256 != *receipt_sha256
        || actual.payload_sha256 != *payload_sha256
        || actual.policy.scope != r.scope
        || actual.policy.capability_graph_revision
            != plan.content.plan.request.capability_graph_revision
        || Some(&actual.replay.package_sha256) != plan.content.package_sha256.get(root)
        || Some(&actual.replay.context_sha256) != plan.content.binding_sha256.get(root)
    {
        return Err("source_capture_binding_mismatch".into());
    }
    Ok(actual)
}
fn source_count(
    capture: &data_captures::CaptureReceipt,
    subject: &str,
    window: &tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow,
    interval: u64,
) -> Result<u64, String> {
    let mut minimum = u64::MAX;
    for batch in capture.replay_response.outputs.values() {
        let mut cursor = window.event_start_ms;
        let mut count = 0;
        for point in batch.points.iter().filter(|p| {
            p.instrument_id == subject
                && p.provenance.event_start_ms >= window.event_start_ms
                && p.provenance.event_end_ms <= window.event_end_ms
        }) {
            let p = &point.provenance;
            if p.event_start_ms != cursor
                || p.event_end_ms.checked_sub(p.event_start_ms) != i64::try_from(interval).ok()
                || !p.completed
                || p.known_at_ms > window.as_of_ms
                || p.arrived_at_ms > window.as_of_ms
                || !matches!(point.result, PointValue::Ready(_))
            {
                return Err("source_history_incomplete".into());
            }
            cursor = p.event_end_ms;
            count += 1;
        }
        if cursor != window.event_end_ms {
            return Err("source_history_incomplete".into());
        }
        minimum = minimum.min(count);
    }
    if minimum == u64::MAX {
        return Err("source_history_incomplete".into());
    }
    Ok(minimum)
}
fn schema_matches(
    s: &TradeAssemblyService,
    capture: &data_captures::CaptureReceipt,
    expected: &SchemaIdentity,
) -> Result<(), String> {
    if capture.acquisition_response.schema_ref != expected.schema_ref {
        return Err("source_schema_mismatch".into());
    }
    let schema = s
        .runtime()
        .plugin_packages
        .data_schema(&capture.acquisition.package_sha256, &expected.schema_ref)?
        .ok_or("source_schema_not_admitted")?;
    if hash(&schema)? != expected.canonical_sha256 {
        return Err("source_schema_mismatch".into());
    }
    Ok(())
}
fn pointer<'a>(payload: &'a Value, path: &str) -> Result<&'a Value, String> {
    payload
        .pointer(path)
        .ok_or("source_field_unavailable".into())
}
fn anchor(
    capture: &data_captures::CaptureReceipt,
    path: &str,
    boundary: &str,
    window: &tradeassembly_plugin_sdk::data_evaluation::EvaluationWindow,
) -> Result<(), String> {
    let value = pointer(&capture.acquisition_response.payload, path)?
        .as_i64()
        .filter(|v| *v > 0 && *v <= MAX_EXACT)
        .ok_or("anchor_invalid")?;
    let expected = match boundary {
        "window_start" => window.event_start_ms,
        "window_end" => window.event_end_ms,
        _ => return Err("anchor_boundary_unsupported".into()),
    };
    if value > window.as_of_ms
        || value != expected
        || capture.acquisition_response.observed_at_ms > window.as_of_ms
        || capture.replay_observed_at_ms > window.as_of_ms
    {
        return Err("anchor_missing_late_or_future".into());
    }
    Ok(())
}
fn output_applicability(
    s: &TradeAssemblyService,
    p: &Published,
    actual: &InstrumentContext,
    mode: &str,
) -> Result<bool, String> {
    let Some(recipe) = p.recipe.as_ref() else {
        return Ok(false);
    };
    // This unprepared negative is definite only for a single-operation recipe.
    // Branch-dependent/mixed models require the actual owned prepared path.
    if recipe.nodes.len() != 1 {
        return Err("candidate_applicability_requires_prepared_path".into());
    }
    let operation = &recipe
        .nodes
        .values()
        .next()
        .ok_or("candidate_operation_missing")?
        .operation;
    if operation.plugin_ref != "self" && operation.plugin_ref != p.package.plugin_ref {
        return Err("candidate_external_operation_unchecked".into());
    }
    let contract = s
        .runtime()
        .plugin_packages
        .operation_data(&p.package.package_sha256, &operation.operation_id)?
        .ok_or("candidate_contract_missing")?;
    let a = &contract.applicability;
    Ok(a.asset_classes.contains(&actual.asset_class)
        && a.instrument_families.contains(&actual.instrument_family)
        && a.modes.iter().any(|m| m == mode))
}
fn grant_checks(
    out: &mut OutputReport,
    fresh: &Value,
    plan: &data_plans::StoredDataPlan,
    required: &BTreeSet<String>,
    valid_until: &mut i64,
    now: i64,
) {
    if fresh["permissionsChecked"] != true {
        out.unknown("entitlement", "current_entitlement_snapshot_unavailable");
        return;
    }
    let Some(nodes) = fresh["graph"]["nodes"].as_array() else {
        out.unknown("entitlement", "current_entitlement_unchecked");
        return;
    };
    let mut complete = true;
    let mut denied = false;
    for id in required {
        let Some(node) = nodes.iter().find(|n| n["nodeId"] == *id) else {
            complete = false;
            continue;
        };
        let old = plan
            .content
            .plan
            .contracts
            .iter()
            .find(|c| c.graph_node_id == *id);
        let Some(selected) = fresh["saved"]["nodes"]
            .as_array()
            .and_then(|ns| ns.iter().find(|n| n["nodeId"] == *id))
            .map(|n| &n["selected"])
        else {
            complete = false;
            continue;
        };
        let candidate = node["candidates"].as_array().and_then(|c| {
            c.iter().find(|c| {
                c["pluginInstanceRef"] == selected["pluginInstanceRef"]
                    && c["operation"]["id"] == selected["operationId"]
            })
        });
        let Some(candidate) = candidate else {
            complete = false;
            continue;
        };
        if old.is_none()
            || selected["manifestFingerprint"]
                != old
                    .map(|c| c.manifest_fingerprint.clone())
                    .unwrap_or_default()
        {
            complete = false;
            continue;
        }
        match candidate["entitlementDecision"]["decision"].as_str() {
            Some("allow") => {}
            Some("deny") => {
                denied = true;
                out.blocked(
                    "entitlement",
                    "current_entitlement_denied",
                    "missing_entitlement",
                );
            }
            _ => complete = false,
        }
        if let Some(expiry) = candidate["entitlementDecision"]["expiresAt"].as_str() {
            match chrono::DateTime::parse_from_rfc3339(expiry) {
                Ok(t) if t.timestamp_millis() > now => {
                    *valid_until = (*valid_until).min(t.timestamp_millis())
                }
                _ => {
                    denied = true;
                    out.blocked(
                        "entitlement",
                        "current_entitlement_expired",
                        "missing_entitlement",
                    );
                }
            }
        }
    }
    if !complete {
        out.unknown("entitlement", "current_entitlement_unchecked");
    } else if !denied {
        out.satisfied(
            "entitlement",
            "current_core_entitlements_checked",
            required.iter().cloned().collect(),
        );
    }
}
fn ancestors(plan: &data_plans::StoredDataPlan, output: &PipelineOutput) -> BTreeSet<String> {
    let mut pending = vec![output.node.clone()];
    let mut visited = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if !visited.insert(node.clone()) {
            continue;
        }
        for b in &plan.content.plan.request.bindings {
            if b.consumer_node == node {
                pending.push(b.producer_node.clone());
            }
        }
    }
    visited
}
struct OutputContext<'a> {
    service: &'a TradeAssemblyService,
    request: &'a Request,
    published: &'a Published,
    plan: &'a data_plans::StoredDataPlan,
    receipt: Option<&'a data_evaluation::EvaluationReceipt>,
    fresh_graph: &'a Value,
    current: bool,
    interval: Option<u64>,
    now: i64,
}
fn assess_output(
    ctx: &OutputContext<'_>,
    binding: &Output,
    origins: &mut BTreeSet<String>,
    valid_until: &mut i64,
) -> Result<Value, String> {
    let s = ctx.service;
    let r = ctx.request;
    let p = ctx.published;
    let plan = ctx.plan;
    let mut out = OutputReport::new(&binding.name);
    out.raw["origin"] = json!(binding.origin);
    out.raw["modelId"] = json!(binding.model_id);
    out.raw["modelVersion"] = json!(binding.model_version);
    out.raw["units"] = json!(binding.units);
    out.raw["schema"] =
        serde_json::to_value(&binding.schema).map_err(|_| "indicator_context_encoding_invalid")?;
    out.satisfied("owner", "current_owner_checked", vec![]);
    out.satisfied(
        "installation",
        "current_verified_package",
        vec![
            p.package.package_sha256.clone(),
            p.package.manifest_sha256.clone(),
        ],
    );
    out.satisfied(
        "profile_binding",
        "publisher_mapping_bound",
        vec![p.binding_hash.clone()],
    );
    if binding.implementation_status == "planned_only" {
        out.blocked(
            "recipe_binding",
            "selected_output_planned_only",
            "planned_only",
        );
        return Ok(out.finish());
    }
    let bound = recipe_matches(plan, r, p);
    if !bound {
        out.unknown("recipe_binding", "prepared_recipe_instance_not_bound");
        match subject(plan, &r.subject_instrument_id)
            .ok()
            .and_then(|(actual, _)| {
                output_applicability(s, p, &actual, &plan.content.plan.request.mode).ok()
            }) {
            Some(false) => out.blocked(
                "instrument",
                "profile_instrument_incompatible",
                "unsupported_instrument",
            ),
            _ => out.unknown("instrument", "candidate_context_not_checked"),
        }
        if ctx.fresh_graph["current"] == true {
            if let Some(nodes) = ctx.fresh_graph["graph"]["nodes"].as_array() {
                for gate in &binding.gates {
                    if let Gate::HostCapture {
                        acquisition_schema, ..
                    } = gate
                    {
                        let admitted = nodes.iter().any(|n| {
                            n["selected"]["operation"]["traits"]["outputSchemaRefs"]
                                .as_array()
                                .is_some_and(|refs| {
                                    refs.iter().any(|v| v == &acquisition_schema.schema_ref)
                                })
                        });
                        if !admitted {
                            out.check(
                                "source_authority",
                                "blocked",
                                "required_schema_not_admitted_in_owned_graph",
                                Some("missing_upstream_capability"),
                                vec![
                                    acquisition_schema.schema_ref.clone(),
                                    acquisition_schema.canonical_sha256.clone(),
                                ],
                                None,
                            );
                        }
                    }
                }
            }
        }
        out.unknown("evaluation", "owned_recipe_evaluation_not_bound");
        return Ok(out.finish());
    }
    out.satisfied(
        "recipe_binding",
        "owned_prepared_recipe_pin_checked",
        vec![plan.revision_id.clone()],
    );
    out.satisfied(
        "schema",
        "actual_recipe_output_schema_checked",
        vec![binding.schema.canonical_sha256.clone()],
    );
    if ctx.current {
        out.satisfied(
            "graph",
            "current_graph_and_plan_checked",
            vec![plan.content.plan.request.capability_graph_revision.clone()],
        );
        out.satisfied("instrument", "owned_literal_context_checked", vec![]);
    } else {
        out.unknown("graph", "current_graph_or_plan_stale");
        out.unknown("instrument", "current_applicability_unchecked");
    }
    let Some(instance) = r.recipe_instance_id.as_deref() else {
        out.unknown("recipe_binding", "recipe_instance_missing");
        return Ok(out.finish());
    };
    let Some(out_link) = resolved(plan, instance, &binding.name) else {
        out.unknown("recipe_binding", "selected_output_not_prepared");
        return Ok(out.finish());
    };
    let root_ids = match roots(plan, &out_link) {
        Ok(roots) => roots,
        Err(_) => {
            out.unknown("source_authority", "source_path_unproven");
            return Ok(out.finish());
        }
    };
    grant_checks(
        &mut out,
        ctx.fresh_graph,
        plan,
        &ancestors(plan, &out_link),
        valid_until,
        ctx.now,
    );
    let window = r.evaluation.as_ref().map(|e| &e.step.window);
    let mut verified_captures = BTreeMap::new();
    let mut covered = BTreeSet::new();
    let mut uses_estimates = false;
    let mut origin_complete = true;
    let mut history_gates = 0;
    let mut gate_inputs = BTreeSet::new();
    for gate in &binding.gates {
        match gate {
            Gate::HostCapture {
                input,
                acquisition_schema,
                origin_pointer,
            } => {
                if !gate_inputs.insert(input) {
                    out.unknown("profile_binding", "duplicate_capture_gate");
                    continue;
                }
                let input_roots = match input_roots(plan, instance, input) {
                    Ok(v) if !v.is_empty() && v.is_subset(&root_ids) => v,
                    _ => {
                        out.unknown("source_authority", "capture_gate_path_unproven");
                        continue;
                    }
                };
                for root in input_roots {
                    let captured = match capture(s, plan, &root, r) {
                        Ok(v) => v,
                        Err(_) => {
                            out.unknown("source_authority", "host_capture_not_current");
                            origin_complete = false;
                            continue;
                        }
                    };
                    if schema_matches(s, &captured, acquisition_schema).is_err() {
                        out.unknown("schema", "capture_schema_not_bound");
                        continue;
                    }
                    match pointer(&captured.acquisition_response.payload, origin_pointer)
                        .ok()
                        .and_then(Value::as_str)
                    {
                        Some(origin @ ("observed" | "estimated" | "simulation" | "synthetic")) => {
                            origins.insert(origin.into());
                            uses_estimates |= origin != "observed";
                        }
                        _ => {
                            out.unknown("input_origin", "input_origin_unproven");
                            origin_complete = false;
                        }
                    }
                    out.satisfied(
                        "source_authority",
                        "actual_host_capture_verified",
                        vec![
                            captured.capture_id.clone(),
                            captured.receipt_sha256.clone(),
                            captured.payload_sha256.clone(),
                        ],
                    );
                    covered.insert(root.clone());
                    verified_captures.insert(root, captured);
                }
            }
            Gate::StrictFixedIntervalHistory {} => history_gates += 1,
            Gate::CaptureAnchor { .. } => {}
        }
    }
    if root_ids.is_empty() || covered != root_ids {
        out.unknown("source_authority", "required_source_path_not_authenticated");
        origin_complete = false;
    }
    if origin_complete {
        out.satisfied("input_origin", "actual_capture_origins_checked", vec![]);
        out.raw["usesEstimatedInputs"] = json!(uses_estimates);
    }
    let mut anchor_gates = 0;
    for gate in &binding.gates {
        if let Gate::CaptureAnchor {
            input,
            anchor_pointer,
            boundary,
        } = gate
        {
            anchor_gates += 1;
            match (window, input_roots(plan, instance, input)) {
                (Some(window), Ok(ids)) if !ids.is_empty() => {
                    for id in ids {
                        match verified_captures.get(&id) {
                            Some(capture)
                                if anchor(capture, anchor_pointer, boundary, window).is_ok() =>
                            {
                                out.satisfied(
                                    "anchor",
                                    "explicit_provider_event_boundary_checked",
                                    vec![capture.capture_id.clone()],
                                )
                            }
                            _ => out.blocked(
                                "anchor",
                                "required_anchor_missing_late_or_future",
                                "missing_anchor",
                            ),
                        }
                    }
                }
                _ => out.unknown("anchor", "anchor_context_unchecked"),
            }
        }
    }
    if anchor_gates == 0 {
        out.check(
            "anchor",
            "not_required",
            "no_anchor_declared",
            None,
            vec![],
            None,
        );
    }
    let mut own_estimate = matches!(binding.estimate, Estimate::Always {});
    if !matches!(
        (binding.origin.as_str(), &binding.estimate),
        ("derived", Estimate::Never {})
            | ("estimated", Estimate::Always {})
            | ("conditional", Estimate::OutputBoolean { .. })
    ) {
        out.unknown("profile_binding", "origin_estimate_contract_mismatch");
    }
    if let Estimate::OutputBoolean { output } = &binding.estimate {
        let flag = resolved(plan, instance, output);
        match (flag, ctx.receipt, window) {
            (Some(flag), Some(receipt), Some(window))
                if roots(plan, &flag).is_ok_and(|v| v.is_subset(&root_ids)) =>
            {
                match last_point(
                    receipt,
                    &flag,
                    &r.subject_instrument_id,
                    window.event_end_ms,
                )
                .map(|p| &p.result)
                {
                    Some(PointValue::Ready(value)) if value.is_boolean() => {
                        own_estimate = value.as_bool().unwrap_or(true)
                    }
                    _ => out.unknown("estimate", "estimate_flag_unproven"),
                }
            }
            _ => out.unknown("estimate", "estimate_flag_unproven"),
        }
    }
    if (own_estimate || uses_estimates) && !r.allow_estimates {
        out.blocked(
            "estimate",
            "explicit_estimate_opt_in_required",
            "estimate_opt_in_required",
        );
    } else {
        out.satisfied(
            "estimate",
            if own_estimate || uses_estimates {
                "explicit_estimate_opt_in_checked"
            } else {
                "profile_and_inputs_not_estimated"
            },
            vec![],
        );
    }
    if history_gates != 1 {
        out.unknown("history", "required_history_gate_not_admitted");
    }
    match (ctx.receipt, window, ctx.interval) {
        (Some(receipt), Some(window), Some(interval)) if history_gates == 1 => {
            out.satisfied(
                "evaluation",
                "original_owned_evaluation_authority_checked",
                vec![
                    receipt.receipt_sha256.clone(),
                    receipt.request_sha256.clone(),
                    receipt.checkpoint.checkpoint_sha256.clone(),
                ],
            );
            if ancestors(plan, &out_link).iter().any(|id| {
                plan.content
                    .plan
                    .contracts
                    .iter()
                    .find(|c| &c.graph_node_id == id)
                    .and_then(|c| c.contract.v3())
                    .is_none_or(|v| {
                        !matches!(
                            v.history.reconstruction,
                            tradeassembly_plugin_sdk::data_history::Reconstruction::FiniteWindow
                        )
                    })
            }) {
                out.unknown("history", "history_origin_or_checkpoint_contract_unproven");
            }
            if receipt.checkpoint.generation != 1
                || r.evaluation
                    .as_ref()
                    .is_some_and(|e| e.step.expected_checkpoint_sha256.is_some())
            {
                out.unknown("history", "checkpoint_reconstruction_unproven");
            } else {
                let required = plan
                    .content
                    .plan
                    .source_history_by_output
                    .get(&out_link.node)
                    .and_then(|v| v.get(&out_link.port));
                if let Some(required) = required {
                    let mut enough = true;
                    for (root, count) in required {
                        if verified_captures.get(root).is_some_and(|c| {
                            c.policy.context.interval_ms != Some(interval)
                                || !c
                                    .policy
                                    .context
                                    .instruments
                                    .iter()
                                    .any(|i| i.instrument_id == r.subject_instrument_id)
                        }) {
                            enough = false;
                            out.unknown("history", "cross_context_history_not_checked");
                            continue;
                        }
                        match verified_captures.get(root).and_then(|c| {
                            source_count(c, &r.subject_instrument_id, window, interval).ok()
                        }) {
                            Some(observed) if observed >= *count => {}
                            _ => {
                                enough = false;
                                out.blocked(
                                    "history",
                                    "selected_output_history_insufficient",
                                    "missing_history",
                                );
                            }
                        }
                    }
                    if enough && !required.is_empty() {
                        out.satisfied(
                            "history",
                            "selected_output_causal_history_checked",
                            required.keys().cloned().collect(),
                        );
                    } else if required.is_empty() {
                        out.unknown("history", "selected_output_history_unproven");
                    }
                } else {
                    out.unknown("history", "per_output_history_contract_not_admitted");
                }
            }
            match last_point(
                receipt,
                &out_link,
                &r.subject_instrument_id,
                window.event_end_ms,
            ) {
                Some(point)
                    if matches!(point.result, PointValue::Ready(_))
                        && point.provenance.completed =>
                {
                    out.raw["temporal"] = temporal(point, window)?;
                    out.satisfied(
                        "clock",
                        "original_event_known_arrival_finality_checked",
                        vec![point.provenance.content_sha256.clone()],
                    );
                }
                Some(point)
                    if matches!(point.result, PointValue::WarmingUp)
                        && out.raw["blockers"].as_array().is_some_and(|bs| {
                            bs.iter().any(|b| b["availability"] == "missing_history")
                        }) =>
                {
                    out.blocked(
                        "evaluation",
                        "selected_output_warming_up",
                        "missing_history",
                    )
                }
                _ => out.unknown("evaluation", "selected_output_not_ready_or_final"),
            }
        }
        _ => {
            out.unknown("evaluation", "evaluation_not_checked");
            out.unknown("history", "actual_output_history_unchecked");
            out.unknown("clock", "actual_output_clock_unchecked");
        }
    }
    Ok(out.finish())
}
/// Inspect existing evidence only. This function never calls an operation port,
/// prepares/saves a plan, advances a run, refreshes health or resolves secret fields.
pub(crate) fn report(s: &TradeAssemblyService, input: &Value) -> Result<Value, String> {
    validate(input, &request_schema(), 65_536)?;
    let r: Request =
        serde_json::from_value(input.clone()).map_err(|_| "indicator_context_request_invalid")?;
    let (who, identity_expiry) = owner(s)?;
    s.require_object("strategy", &r.scope.strategy_id)
        .map_err(|_| "indicator_context_not_available")?;
    let plan = data_plans::read_owned(s, &r.plan_revision_id)?;
    if plan.content.scope != r.scope {
        return Err("indicator_context_scope_mismatch".into());
    }
    if let Some(context) = s.agent_mcp_execution_context() {
        // Read-only inspection keeps the durable runner lease and actual plan
        // mode fence; caller selectors never expand the delegated context.
        if context.mode() != plan.content.plan.request.mode {
            return Err("agent_mcp_execution_context_mismatch".into());
        }
        let (_, principal) = s
            .responsible_human_identity()
            .ok_or("indicator_context_owner_required")?;
        crate::agent_runner::bind_mcp_execution_context(
            s.runtime().as_ref(),
            context,
            TOOL,
            json!({"mode":plan.content.plan.request.mode,"authority_context":{"actor":principal,"accountMode":plan.content.plan.request.mode}}),
        )?;
    }
    let (actual, interval) = subject(&plan, &r.subject_instrument_id)?;
    let inspected = s.runtime().clock.trusted_now_ms()?;
    let mut valid_until = inspected.saturating_add(30_000).min(identity_expiry);
    let raw_graph = s
        .runtime()
        .storage
        .get_json(
            capability_graph::REVISIONS_NS,
            &plan.content.plan.request.capability_graph_revision,
        )?
        .ok_or("indicator_context_graph_missing")?;
    if raw_graph["strategyId"] != r.scope.strategy_id
        || raw_graph["strategyVersionId"] != r.scope.strategy_version_id
        || raw_graph["strategySpecHash"] != r.scope.strategy_spec_hash
    {
        return Err("indicator_context_graph_scope_mismatch".into());
    }
    let mut fresh = capability_graph::check_current(
        s,
        json!({"revisionId":plan.content.plan.request.capability_graph_revision,"evaluationEpoch":chrono::DateTime::from_timestamp_millis(inspected).ok_or("indicator_context_clock_invalid")?.to_rfc3339()}),
    );
    // Preserve exact original selection solely to inspect a current denial; a
    // denied candidate is deliberately absent from the fresh selected field.
    fresh["saved"] = raw_graph["graph"].clone();
    let permissions = permission_snapshot(s);
    fresh["permissionsChecked"] = json!(permissions.is_ok());
    let current = data_plans::current(s, &r.plan_revision_id).is_ok_and(|p| p == plan)
        && fresh["current"] == true
        && permissions.is_ok();
    let evaluation = if current {
        original_evaluation(s, &r, &plan).ok()
    } else {
        None
    };
    let publisher = published(s, &r);
    let mut origins = BTreeSet::new();
    let (mut outputs, resolved, mut axes) = match &publisher {
        Ok(p) => {
            let context = OutputContext {
                service: s,
                request: &r,
                published: p,
                plan: &plan,
                receipt: evaluation.as_ref(),
                fresh_graph: &fresh,
                current,
                interval,
                now: inspected,
            };
            let mut reports = Vec::new();
            for name in &r.selection.selected_outputs {
                let selected = p
                    .profile
                    .outputs
                    .iter()
                    .find(|o| &o.name == name)
                    .ok_or("indicator_context_output_not_admitted")?;
                reports.push(assess_output(
                    &context,
                    selected,
                    &mut origins,
                    &mut valid_until,
                )?);
            }
            let resolved = json!({"packageSha256":p.package.package_sha256,"manifestSha256":p.package.manifest_sha256,"bindingCanonicalSha256":p.binding_hash,
                "catalog":p.binding.catalog,"conceptId":p.profile.concept_id,"profileId":p.profile.profile_id,"profileVersion":p.profile.profile_version,"recipe":p.profile.recipe});
            let host_bound = evaluation.is_some()
                && recipe_matches(&plan, &r, p)
                && reports.iter().any(|out| {
                    out["checks"].as_array().is_some_and(|checks| {
                        checks
                            .iter()
                            .any(|c| c["check"] == "evaluation" && c["state"] == "satisfied")
                    })
                });
            (
                reports,
                resolved,
                json!({"mapping":"publisher_bound","sdkAdmission":if current&&p.recipe.is_some(){"verified_current"}else{"not_checked"},"hostExecution":if host_bound{"owned_committed_evaluation"}else{"not_checked"},
                "sourceAuthority":"not_checked","sourceOrigins":[],"publisherProcessClaims":"not_used_as_host_evidence","vendorEquivalence":"not_checked"}),
            )
        }
        Err(code) => {
            let safe = match code.as_str() {
                "catalog_not_admitted" => "catalog_not_admitted",
                "indicator_context_binding_contract_unsupported" => "binding_contract_unsupported",
                _ => "profile_or_installation_not_current",
            };
            let reports = r
                .selection
                .selected_outputs
                .iter()
                .map(|name| {
                    let mut out = OutputReport::new(name);
                    out.unknown("profile_binding", safe);
                    out.finish()
                })
                .collect();
            (
                reports,
                Value::Null,
                json!({"mapping":if safe=="catalog_not_admitted"{"not_admitted"}else{"not_checked"},"sdkAdmission":"not_checked","hostExecution":"not_checked","sourceAuthority":"not_checked","sourceOrigins":[],"publisherProcessClaims":"not_used_as_host_evidence","vendorEquivalence":"not_checked"}),
            )
        }
    };
    let now = s.runtime().clock.trusted_now_ms()?;
    if outputs.iter().any(|o| o["availability"] == "available") {
        // No stale-owner/source/permission snapshot can be sealed as available.
        let rechecked = owner(s).is_ok_and(|(owner, expiry)| owner == who && expiry > now)
            && data_plans::current(s, &r.plan_revision_id).is_ok_and(|p| p == plan)
            && permission_snapshot(s)
                .is_ok_and(|v| permissions.as_ref().is_ok_and(|before| before == &v))
            && published(s, &r).is_ok_and(|p| {
                publisher.as_ref().is_ok_and(|before| {
                    before.binding_hash == p.binding_hash && before.package == p.package
                })
            });
        if !rechecked || now >= valid_until {
            for output in &mut outputs {
                if output["availability"] == "available" {
                    let mut out = OutputReport {
                        raw: output.clone(),
                    };
                    out.unknown("clock", "inspection_bindings_changed_or_expired");
                    *output = out.finish();
                }
            }
            axes["sdkAdmission"] = json!("not_checked");
        }
    }
    axes["sourceOrigins"] = json!(origins);
    if outputs.iter().any(|o| {
        o["checks"].as_array().is_some_and(|checks| {
            checks
                .iter()
                .any(|c| c["check"] == "source_authority" && c["state"] == "satisfied")
        })
    }) {
        axes["sourceAuthority"] = json!("host_acquisition");
    }
    if !outputs.iter().any(|o| o["availability"] == "available") {
        valid_until = inspected;
    }
    let policy = permissions
        .as_ref()
        .ok()
        .map(|grants| hash(&json!({"grants":grants,"currentGraph":fresh["graph"]})))
        .transpose()?;
    let mut result = json!({"schemaVersion":"tradeassembly.indicator_context_support.v1","requestSha256":hash(&r)?,"reportSha256":"",
        "scope":r.scope,"ownerSha256":who,"coreRevision":crate::build_identity::current().ok_or("indicator_context_build_identity_required")?.revision(),
        "capabilityGraphRevisionId":plan.content.plan.request.capability_graph_revision,"capabilityGraphFingerprint":plan.content.plan.capability_graph_fingerprint,
        "policySnapshotSha256":policy,"subject":{"instrumentId":actual.instrument_id,"assetClass":actual.asset_class,"instrumentFamily":actual.instrument_family,"mode":plan.content.plan.request.mode,"intervalMs":interval},
        "inspectedAtMs":inspected,"validUntilMs":valid_until,"resolved":resolved,"axes":axes,"outputs":outputs,
        "executionAuthorityGranted":false,"providerCalls":0,"persistentWrites":0});
    result["reportSha256"] = json!(hash(&result)?);
    validate(&result, &response_schema(), MAX_REPORT)?;
    Ok(result)
}
pub(crate) fn inspect(s: &TradeAssemblyService, input: &Value) -> ServiceResponse {
    match report(s, input) {
        Ok(result) => ServiceResponse::ok(result),
        Err(code) if code == "indicator_context_not_available" => {
            ServiceResponse::object_unavailable()
        }
        Err(code) => ServiceResponse::bad_request(&code),
    }
}

#[cfg(test)]
mod tests;
