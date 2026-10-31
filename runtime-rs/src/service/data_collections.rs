//! Host-witnessed bounded document acquisition for instrument selection.
//! Unlike bar captures this assigns no artificial candle interval, replay
//! observations, global coverage or historical publication time.
use super::{
    capability_graph, data_binding, data_captures, data_evaluation, data_plans::PlanScope,
    TradeAssemblyService,
};
use crate::{data_pipeline::canonical_sha256, ports::*};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const RECEIPTS: &str = "data_collection_receipts_v1";
const KEYS: &str = "data_collection_keys_v1";
pub const MAX_COLLECTION_BYTES: usize = 524_288;
const MAX_RECEIPT_BYTES: usize = 655_360;
const CREATE_PATH: &str = "/product/strategies/data-collections/create";
const GET_PATH: &str = "/product/strategies/data-collections/get";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionCommand {
    pub scope: PlanScope,
    pub capability_graph_revision: String,
    pub acquisition_node: String,
    pub mode: String,
    /// Exact verified response schema, not a caller-supplied schema document.
    pub response_schema_ref: String,
    pub input: Value,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionVersion {
    #[serde(rename = "tradeassembly.data_collection_receipt.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionReceipt {
    pub schema_version: CollectionVersion,
    pub collection_id: String,
    pub receipt_sha256: String,
    pub command: CollectionCommand,
    pub acquisition: data_binding::PinnedOperation,
    pub response_schema_sha256: String,
    pub response: PluginOperationResponse,
    pub payload_sha256: String,
    /// Host witness; never substituted for a field's publication/effective date.
    pub host_known_at_ms: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionSummaryVersion {
    #[serde(rename = "tradeassembly.data_collection_summary.v1")]
    V1,
}
/// Stable public evidence reference. Raw queries/pages/cursors/account bindings
/// stay in owner-local storage; generic command redaction cannot alter evidence
/// bytes inside this summary on a cached response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionSummary {
    pub schema_version: CollectionSummaryVersion,
    pub collection_id: String,
    pub receipt_sha256: String,
    pub scope: PlanScope,
    pub capability_graph_revision: String,
    pub mode: String,
    pub plugin_ref: String,
    pub operation_id: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub response_schema_ref: String,
    pub response_schema_sha256: String,
    pub payload_sha256: String,
    pub observed_at_ms: i64,
    pub host_known_at_ms: i64,
}
fn summary(r: &CollectionReceipt) -> CollectionSummary {
    CollectionSummary {
        schema_version: CollectionSummaryVersion::V1,
        collection_id: r.collection_id.clone(),
        receipt_sha256: r.receipt_sha256.clone(),
        scope: r.command.scope.clone(),
        capability_graph_revision: r.command.capability_graph_revision.clone(),
        mode: r.command.mode.clone(),
        plugin_ref: r.acquisition.plugin_ref.clone(),
        operation_id: r.acquisition.operation_id.clone(),
        package_sha256: r.acquisition.package_sha256.clone(),
        manifest_sha256: r.acquisition.manifest_sha256.clone(),
        response_schema_ref: r.command.response_schema_ref.clone(),
        response_schema_sha256: r.response_schema_sha256.clone(),
        payload_sha256: r.payload_sha256.clone(),
        observed_at_ms: r.response.observed_at_ms,
        host_known_at_ms: r.host_known_at_ms,
    }
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "data_collection_encoding_invalid".into())
}
fn identity(receipt: &CollectionReceipt) -> Result<String, String> {
    let mut value = receipt.clone();
    value.collection_id.clear();
    value.receipt_sha256.clear();
    hash(&value)
}
/// A document-specific work budget, independent of package/schema metadata
/// limits. Transport bounds remain unchanged. No slicing/truncation occurs.
fn document_budget(value: &Value) -> Result<(), String> {
    if serde_json::to_vec(value)
        .map_err(|_| "data_collection_encoding_invalid")?
        .len()
        > MAX_COLLECTION_BYTES
    {
        return Err("data_collection_document_too_large".into());
    }
    let mut stack = vec![(value, 0)];
    let mut visited = 0;
    while let Some((item, depth)) = stack.pop() {
        visited += 1;
        if visited > 65_536 || depth > 24 {
            return Err("data_collection_document_bounds_invalid".into());
        }
        match item {
            Value::Array(rows) => {
                if rows.len() > 4096 {
                    return Err("data_collection_document_bounds_invalid".into());
                }
                stack.extend(rows.iter().map(|v| (v, depth + 1)));
            }
            Value::Object(fields) => {
                if fields.len() > 256 || fields.keys().any(|k| k.len() > 256) {
                    return Err("data_collection_document_bounds_invalid".into());
                }
                stack.extend(fields.values().map(|v| (v, depth + 1)));
            }
            Value::String(s) if s.len() > 16_384 => {
                return Err("data_collection_document_bounds_invalid".into())
            }
            _ => {}
        }
    }
    data_captures::capturable_payload(value)?;
    hash(value)?;
    Ok(())
}
fn resolve(
    service: &TradeAssemblyService,
    c: &CollectionCommand,
) -> Result<
    (
        CapabilityGraphResolution,
        CapabilitySelection,
        data_binding::PinnedOperation,
    ),
    String,
> {
    service
        .require_object("strategy", &c.scope.strategy_id)
        .map_err(|_| "data_collection_not_available")?;
    if !matches!(c.mode.as_str(), "backtest" | "paper" | "live")
        || !c.input.is_object()
        || c.input.get("config").is_some()
        || c.response_schema_ref.is_empty()
        || c.response_schema_ref.len() > 256
        || serde_json::to_vec(c)
            .map_err(|_| "data_collection_input_invalid")?
            .len()
            > 65536
    {
        return Err("data_collection_command_invalid".into());
    }
    tradeassembly_plugin_sdk::data_package::metadata_budget(&c.input)?;
    data_captures::capturable_payload(&c.input)?;
    let now = chrono::DateTime::from_timestamp_millis(service.runtime().clock.now_ms())
        .ok_or("data_collection_clock_invalid")?
        .to_rfc3339();
    let revision = capability_graph::revision_for_context(
        service,
        &c.capability_graph_revision,
        &c.scope.strategy_id,
        &c.scope.strategy_version_id,
        &c.mode,
        &now,
    )
    .map_err(|_| "data_collection_graph_stale")?;
    if revision["strategySpecHash"] != c.scope.strategy_spec_hash {
        return Err("data_collection_strategy_mismatch".into());
    }
    let graph: CapabilityGraphResolution = serde_json::from_value(revision["graph"].clone())
        .map_err(|_| "data_collection_graph_invalid")?;
    let selected = graph
        .nodes
        .iter()
        .find(|n| n.node_id == c.acquisition_node)
        .and_then(|n| n.selected.clone())
        .ok_or("data_collection_node_unresolved")?;
    let pin = data_binding::pin(service, &selected)?;
    let package = service
        .runtime()
        .plugin_packages
        .get(&pin.package_sha256)?
        .ok_or("data_collection_package_missing")?;
    data_evaluation::allowed_data_operation(&selected, false, &package.manifest)?;
    if !selected.operation.traits.modes.contains(&c.mode)
        || !selected
            .operation
            .traits
            .output_schema_refs
            .contains(&c.response_schema_ref)
    {
        return Err("data_collection_context_unsupported".into());
    }
    Ok((graph, selected, pin))
}
fn schema(
    service: &TradeAssemblyService,
    pin: &data_binding::PinnedOperation,
    schema_ref: &str,
) -> Result<Value, String> {
    service
        .runtime()
        .plugin_packages
        .data_schema(&pin.package_sha256, schema_ref)?
        .ok_or("data_collection_response_schema_missing".into())
}
fn validate_response(service: &TradeAssemblyService, r: &CollectionReceipt) -> Result<(), String> {
    document_budget(
        &serde_json::to_value(&r.response).map_err(|_| "data_collection_encoding_invalid")?,
    )?;
    document_budget(&r.response.payload)?;
    if r.response.schema_ref != r.command.response_schema_ref
        || r.response.reconciliation_required
        || r.response.freshness_state != "fresh"
        || r.response.source_event_id.is_empty()
        || r.response.content_hash.is_empty()
        || r.response.observed_at_ms <= 0
        || r.host_known_at_ms < r.response.observed_at_ms
        || r.host_known_at_ms > service.runtime().clock.now_ms()
        || hash(&r.response.payload)? != r.payload_sha256
    {
        return Err("data_collection_response_invalid".into());
    }
    let s = schema(service, &r.acquisition, &r.command.response_schema_ref)?;
    if hash(&s)? != r.response_schema_sha256
        || !jsonschema::validator_for(&s)
            .map_err(|_| "data_collection_response_schema_invalid")?
            .is_valid(&r.response.payload)
    {
        return Err("data_collection_response_schema_invalid".into());
    }
    Ok(())
}
/// Validates provenance/current bindings, not eligibility/freshness for entries.
/// A downstream selector must apply its explicit coverage and field-age policy.
pub fn current(service: &TradeAssemblyService, id: &str) -> Result<CollectionReceipt, String> {
    let value = service
        .runtime()
        .storage
        .get_json(RECEIPTS, id)?
        .ok_or("data_collection_missing")?;
    if serde_json::to_vec(&value)
        .map_err(|_| "data_collection_receipt_invalid")?
        .len()
        > MAX_RECEIPT_BYTES
    {
        return Err("data_collection_receipt_invalid".into());
    }
    let receipt: CollectionReceipt =
        serde_json::from_value(value).map_err(|_| "data_collection_receipt_invalid")?;
    if receipt.collection_id != id
        || identity(&receipt)? != receipt.receipt_sha256
        || id != format!("collection_{}", receipt.receipt_sha256)
    {
        return Err("data_collection_receipt_invalid".into());
    }
    let (_, _, pin) = resolve(service, &receipt.command)?;
    if pin != receipt.acquisition {
        return Err("data_collection_binding_stale".into());
    }
    validate_response(service, &receipt)?;
    Ok(receipt)
}
/// Acquires through the existing scoped operation port. Never accepts a raw
/// caller response, guesses provider coverage or invokes a broker operation.
pub fn acquire(
    service: &TradeAssemblyService,
    command: CollectionCommand,
    context: &SideEffectContext,
) -> Result<CollectionReceipt, String> {
    let (graph, selected, pin) = resolve(service, &command)?;
    validate_context(&command, context)?;
    let command_sha = hash(&command)?;
    let key = hash(
        &json!({"strategy":command.scope.strategy_id,"authority":context.authority,"key":context.idempotency_key.as_str()}),
    )?;
    if let Some(previous) = service.runtime().storage.get_json(KEYS, &key)? {
        if previous["commandSha256"] != command_sha {
            return Err("data_collection_idempotency_conflict".into());
        }
        return current(
            service,
            previous["collectionId"]
                .as_str()
                .ok_or("data_collection_receipt_invalid")?,
        );
    }
    let correlation = hash(&json!({"command":command_sha,"key":key}))?;
    let request = PluginOperationRequest {
        correlation_id: correlation.clone(),
        plugin_instance_ref: selected.plugin_instance_ref.clone(),
        plugin_ref: selected.plugin_ref.clone(),
        manifest_fingerprint: selected.manifest_fingerprint.clone(),
        operation_id: selected.operation_id.clone(),
        capability: selected.operation.capability.clone(),
        capability_graph_revision_id: command.capability_graph_revision.clone(),
        capability_graph_fingerprint: graph.graph_fingerprint,
        strategy_id: command.scope.strategy_id.clone(),
        strategy_version_id: command.scope.strategy_version_id.clone(),
        strategy_spec_hash: command.scope.strategy_spec_hash.clone(),
        activation_id: String::new(),
        attempt_id: correlation.clone(),
        evaluation_tick_id: correlation.clone(),
        mode: command.mode.clone(),
        purpose: selected.operation.purpose.clone(),
        account_ref: selected.account_ref.clone(),
        timeout_ms: 15000,
        fencing_token: None,
        input: command.input.clone(),
        evidence_refs: vec![],
    };
    let mut call = context.clone();
    call.idempotency_key = IdempotencyKey::new(format!("data-collection:{correlation}"))?;
    let response = service
        .runtime()
        .plugin_operations
        .invoke(&request, &call)?;
    document_budget(
        &serde_json::to_value(&response).map_err(|_| "data_collection_encoding_invalid")?,
    )?;
    if response.correlation_id != correlation {
        return Err("data_collection_response_correlation_invalid".into());
    }
    let mut receipt = CollectionReceipt {
        schema_version: CollectionVersion::V1,
        collection_id: String::new(),
        receipt_sha256: String::new(),
        command,
        acquisition: pin,
        response_schema_sha256: hash(&schema(
            service,
            &data_binding::pin(service, &selected)?,
            &response.schema_ref,
        )?)?,
        payload_sha256: hash(&response.payload)?,
        response,
        host_known_at_ms: service.runtime().clock.now_ms(),
    };
    validate_response(service, &receipt)?;
    if resolve(service, &receipt.command)?.2 != receipt.acquisition {
        return Err("data_collection_binding_stale".into());
    }
    receipt.receipt_sha256 = identity(&receipt)?;
    receipt.collection_id = format!("collection_{}", receipt.receipt_sha256);
    if serde_json::to_vec(&receipt)
        .map_err(|_| "data_collection_encoding_invalid")?
        .len()
        > MAX_RECEIPT_BYTES
    {
        return Err("data_collection_receipt_too_large".into());
    }
    let writes = vec![
        StorageWrite::new(
            RECEIPTS,
            &receipt.collection_id,
            serde_json::to_value(&receipt).map_err(|_| "data_collection_encoding_invalid")?,
            context.clone(),
        ),
        StorageWrite::new(
            KEYS,
            &key,
            json!({"collectionId":receipt.collection_id,"commandSha256":command_sha}),
            context.clone(),
        ),
    ];
    if service.runtime().storage.put_json_batch(
        &writes,
        &[
            StorageExpectation::new(RECEIPTS, &receipt.collection_id, None),
            StorageExpectation::new(KEYS, &key, None),
        ],
    )? != ComparePutOutcome::Updated
    {
        return Err("data_collection_commit_conflict".into());
    }
    Ok(receipt)
}

fn context(service: &TradeAssemblyService, body: &Value) -> Result<SideEffectContext, String> {
    let envelope = crate::control_plane::http_envelope("http", "POST", CREATE_PATH, body)?;
    Ok(SideEffectContext::new(
        envelope.authority,
        IdempotencyKey::new(
            body["idempotencyKey"]
                .as_str()
                .ok_or("data_collection_idempotency_required")?,
        )?,
    )
    .with_agent_execution(service.agent_mcp_execution_context()))
}
fn validate_context(
    command: &CollectionCommand,
    context: &SideEffectContext,
) -> Result<(), String> {
    let expected = if command.mode == "live" {
        "live"
    } else {
        "paper"
    };
    if context.authority.account_mode != expected {
        return Err("data_collection_mode_mismatch".into());
    }
    Ok(())
}
fn command(body: &Value) -> Result<CollectionCommand, String> {
    serde_json::from_value(body["collection"].clone())
        .map_err(|_| "data_collection_command_invalid".into())
}
fn error(code: &str) -> super::ServiceResponse {
    if matches!(
        code,
        "data_collection_command_invalid"
            | "data_collection_idempotency_required"
            | "data_collection_id_required"
            | "data_collection_not_available"
    ) {
        super::ServiceResponse::bad_request(code)
    } else {
        super::ServiceResponse::conflict(code)
    }
}
pub fn create_response(service: &TradeAssemblyService, body: Value) -> super::ServiceResponse {
    let result = (|| acquire(service, command(&body)?, &context(service, &body)?))();
    match result {
        Ok(receipt) => super::ServiceResponse::ok(
            json!({"collection":summary(&receipt),"executionAuthorityGranted":false}),
        ),
        Err(code) => error(&code),
    }
}
pub fn read_response(service: &TradeAssemblyService, body: Value) -> super::ServiceResponse {
    let result = body["collectionId"]
        .as_str()
        .ok_or("data_collection_id_required".to_string())
        .and_then(|id| current(service, id).map_err(|_| "data_collection_not_available".into()));
    match result {
        Ok(receipt) => super::ServiceResponse::ok(
            json!({"collection":summary(&receipt),"executionAuthorityGranted":false}),
        ),
        Err(code) => error(&code),
    }
}
/// Executed before the outer command cache, including MCP retries.
pub(crate) fn preflight(
    service: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Option<super::ServiceResponse> {
    let result: Result<(), String> = (|| {
        match path {
            CREATE_PATH => {
                let c = command(body)?;
                resolve(service, &c)?;
                let ctx = context(service, body)?;
                validate_context(&c, &ctx)?;
                let key = hash(
                    &json!({"strategy":c.scope.strategy_id,"authority":ctx.authority,"key":ctx.idempotency_key.as_str()}),
                )?;
                if let Some(previous) = service.runtime().storage.get_json(KEYS, &key)? {
                    if previous["commandSha256"] != hash(&c)? {
                        return Err("data_collection_idempotency_conflict".into());
                    }
                    current(
                        service,
                        previous["collectionId"]
                            .as_str()
                            .ok_or("data_collection_receipt_invalid")?,
                    )?;
                }
            }
            GET_PATH => {
                current(
                    service,
                    body["collectionId"]
                        .as_str()
                        .ok_or("data_collection_id_required")?,
                )
                .map_err(|_| "data_collection_not_available")?;
            }
            _ => {}
        }
        Ok(())
    })();
    if !matches!(path, CREATE_PATH | GET_PATH) {
        return None;
    }
    result.err().map(|code| error(&code))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn document_limits_reject_whole_result_without_truncation() {
        let rows = json!((0..4096).collect::<Vec<_>>());
        document_budget(&rows).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 4096);
        assert_eq!(
            document_budget(&json!((0..4097).collect::<Vec<_>>())),
            Err("data_collection_document_bounds_invalid".into())
        );
        assert_eq!(
            document_budget(&json!(["x".repeat(MAX_COLLECTION_BYTES)])),
            Err("data_collection_document_too_large".into())
        );
        let mut deep = json!(0);
        for _ in 0..25 {
            deep = json!([deep]);
        }
        assert_eq!(
            document_budget(&deep),
            Err("data_collection_document_bounds_invalid".into())
        );
        assert!(document_budget(&json!({"headers":{"test":"must-not-persist"}})).is_err());
    }
}
