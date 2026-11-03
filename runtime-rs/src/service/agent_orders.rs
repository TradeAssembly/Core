//! Explicit broker-order tools for an attached external-agent MCP connection.
//! Do not route arbitrary plugin requests from an MCP caller through this module.

use super::{broker_recovery, TradeAssemblyService};
use crate::broker_order_intent::CanonicalBrokerOrder;
use crate::ports::{AuthorityContext, IdempotencyKey, PluginOperationRequest, SideEffectContext};
use serde_json::{json, Value};

pub(super) fn submit(service: &TradeAssemblyService, arguments: Value) -> Value {
    match submit_bound(service, &arguments) {
        Ok(value) => value,
        Err(code) => json!({"error":{"code":code}}),
    }
}

fn submit_bound(service: &TradeAssemblyService, arguments: &Value) -> Result<Value, &'static str> {
    let agent = service
        .agent_mcp_execution_context()
        .ok_or("agent_order_attachment_required")?;
    if !matches!(agent.mode(), "paper" | "live") {
        return Err("agent_order_mode_unsupported");
    }
    let (submit_capability, submit_operation, purpose) = if agent.mode() == "paper" {
        (
            "broker.order_submit.paper",
            "broker.paper_order_submit",
            "paper_trading",
        )
    } else {
        (
            "broker.order_submit.live",
            "broker.live_order_submit",
            "live_order_submission",
        )
    };
    let activation_id = text(arguments, "activation_id")?;
    let instance_ref = text(arguments, "plugin_instance_ref")?;
    let key = IdempotencyKey::new(text(arguments, "idempotency_key")?)
        .map_err(|_| "agent_order_idempotency_key_invalid")?;
    let order =
        CanonicalBrokerOrder::from_input(&arguments["order"]).map_err(|_| "agent_order_invalid")?;
    let runtime = service.runtime();
    agent
        .current_lease(
            runtime.storage.as_ref(),
            runtime.clock.as_ref(),
            runtime.leases.as_ref(),
        )
        .map_err(|_| "agent_order_lease_invalid")?;
    let attached_run = runtime
        .storage
        .get_json(crate::agent_runner::RUNS_NS, agent.run_id())
        .map_err(|_| "agent_order_state_unavailable")?
        .ok_or("agent_order_run_missing")?;
    if attached_run["activationId"] != activation_id {
        return Err("agent_order_activation_mismatch");
    }
    let run = runtime
        .storage
        .get_json("execution_runs", &format!("run_{activation_id}"))
        .map_err(|_| "agent_order_state_unavailable")?
        .ok_or("agent_order_execution_missing")?;
    if run["activationId"] != activation_id
        || run["orchestrator"] != "external_agent"
        || run["mode"] != agent.mode()
    {
        return Err("agent_order_execution_mismatch");
    }
    let revision_id = field(&run, "capabilityGraphRevisionId")?;
    let revision = runtime
        .storage
        .get_json("capability_graph_revisions", revision_id)
        .map_err(|_| "agent_order_state_unavailable")?
        .ok_or("agent_order_revision_missing")?;
    let selected = revision["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node["blockers"].as_array().is_some_and(Vec::is_empty))
        .find_map(|node| {
            (node["requirement"]["capability"] == submit_capability
                && node["selected"]["operationId"] == submit_operation
                && node["selected"]["pluginInstanceRef"] == instance_ref)
                .then(|| &node["selected"])
        })
        .ok_or("agent_order_binding_missing")?;
    let input = json!({
        "symbol": &order.symbol,
        "side": &order.side,
        "orderType": &order.order_type,
        "timeInForce": &order.time_in_force,
        "clientOrderId": &order.client_order_id,
        "quantity": order.canonical_quantity(),
    });
    let mut request = PluginOperationRequest {
        correlation_id: run["correlationId"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("activation:{activation_id}")),
        plugin_instance_ref: field(selected, "pluginInstanceRef")?.into(),
        plugin_ref: field(selected, "pluginRef")?.into(),
        manifest_fingerprint: field(selected, "manifestFingerprint")?.into(),
        operation_id: submit_operation.into(),
        capability: submit_capability.into(),
        capability_graph_revision_id: revision_id.into(),
        capability_graph_fingerprint: field(&run, "capabilityGraphFingerprint")?.into(),
        strategy_id: field(&run, "strategyId")?.into(),
        strategy_version_id: field(&run, "strategyVersionId")?.into(),
        strategy_spec_hash: field(&run, "strategySpecHash")?.into(),
        activation_id: activation_id.into(),
        attempt_id: format!("agent-order:{}", key.as_str()),
        evaluation_tick_id: format!("agent-order:{}", key.as_str()),
        mode: agent.mode().into(),
        purpose: purpose.into(),
        account_ref: selected["accountRef"].as_str().map(str::to_string),
        timeout_ms: 10_000,
        fencing_token: None,
        input,
        evidence_refs: vec![],
    };
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: format!("agent-deployment:{}", agent.deployment_id()),
            surface: "agent_order".into(),
            account_mode: agent.mode().into(),
        },
        key,
    )
    .with_agent_execution(Some(agent));
    let quote_selected = revision["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node["blockers"].as_array().is_some_and(Vec::is_empty))
        .find_map(|node| {
            (node["requirement"]["capability"] == "marketdata.quote"
                && node["selected"]["operationId"] == "marketdata.quote.read")
                .then(|| &node["selected"])
        })
        .ok_or("agent_order_quote_binding_missing")?;
    let quote_key =
        IdempotencyKey::new(format!("agent-quote:{}", context.idempotency_key.as_str()))
            .map_err(|_| "agent_order_quote_key_invalid")?;
    let quote_request = PluginOperationRequest {
        plugin_instance_ref: field(quote_selected, "pluginInstanceRef")?.into(),
        plugin_ref: field(quote_selected, "pluginRef")?.into(),
        manifest_fingerprint: field(quote_selected, "manifestFingerprint")?.into(),
        operation_id: "marketdata.quote.read".into(),
        capability: "marketdata.quote".into(),
        purpose: "market_data_research".into(),
        account_ref: quote_selected["accountRef"].as_str().map(str::to_string),
        attempt_id: format!("agent-quote:{}", context.idempotency_key.as_str()),
        evaluation_tick_id: format!("agent-quote:{}", context.idempotency_key.as_str()),
        input: json!({
            "symbol":order.symbol,
            "assetClass":if run["assetClass"] == "crypto_spot" {"crypto"} else {"equity"},
        }),
        evidence_refs: vec![],
        ..request.clone()
    };
    let quote_context = SideEffectContext::new(context.authority.clone(), quote_key.clone())
        .with_agent_execution(Some(agent));
    let quote = runtime
        .plugin_operations
        .invoke(&quote_request, &quote_context)
        .map_err(|_| "agent_order_quote_unavailable")?;
    if quote.reconciliation_required {
        return Err("agent_order_quote_unavailable");
    }
    request.evidence_refs = vec![format!("plugin-receipt:{}", quote_key.as_str())];
    match runtime.plugin_operations.invoke(&request, &context) {
        Ok(receipt) if receipt.reconciliation_required => {
            Err("agent_order_reconciliation_required")
        }
        Ok(receipt) => Ok(json!({
            "status":"submitted",
            "originalIdempotencyKey":context.idempotency_key.as_str(),
            "receipt":receipt,
        })),
        Err(error) if error == "plugin_operation_idempotency_conflict" => {
            Err("agent_order_idempotency_conflict")
        }
        Err(error) if error == "plugin_order_reconciliation_required" => {
            Err("agent_order_reconciliation_required")
        }
        Err(_) => Err("agent_order_submission_denied"),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    value[key]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or("agent_order_argument_required")
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    value[key]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or("agent_order_binding_invalid")
}

pub(super) fn reconcile(service: &TradeAssemblyService, arguments: Value) -> Value {
    let Some(agent) = service.agent_mcp_execution_context() else {
        return json!({"error":{"code":"agent_order_attachment_required"}});
    };
    if !matches!(agent.mode(), "paper" | "live") {
        return json!({"error":{"code":"agent_order_recovery_mode_unsupported"}});
    }
    let body = json!({
        "originalIdempotencyKey": arguments["original_idempotency_key"],
        "recoveryIdempotencyKey": arguments["idempotency_key"],
    });
    broker_recovery::recover(service, body).body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_order_bare_connection_cannot_submit_or_reconcile() {
        let dir = tempfile::tempdir().expect("isolated test directory");
        let db = dir.path().join("runtime.db");
        let service = TradeAssemblyService::test_local(db.to_string_lossy());
        assert_eq!(
            submit(&service, json!({}))["error"]["code"],
            "agent_order_attachment_required"
        );
        assert_eq!(
            reconcile(&service, json!({}))["error"]["code"],
            "agent_order_attachment_required"
        );
        assert!(service
            .runtime()
            .storage
            .list_json("plugin_broker_dispatch_claims")
            .expect("dispatch claims")
            .is_empty());
    }
}
