use super::{providers, TradeAssemblyService};
use crate::capability::now_rfc3339_second;
use crate::ports::{
    AuthorityContext, CapabilityBinding, CapabilityGraphRequest, CapabilityGraphResolution,
    CapabilityRequirement, CapabilityResolverCatalog, IdempotencyKey, SideEffectContext,
};
use crate::spec::StrategySpec;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) const REVISIONS_NS: &str = "capability_graph_revisions";
pub(crate) const IDEMPOTENCY_NS: &str = "capability_graph_idempotency";
const REVISION_SCHEMA_VERSION: &str = "tradeassembly.capability_graph_revision.v1";

pub(crate) fn resolve(service: &TradeAssemblyService, body: Value) -> Value {
    match graph_input(service, &body) {
        Ok(input) => graph_value(service, &input.request),
        Err(error) => error,
    }
}

pub(crate) fn save_revision(service: &TradeAssemblyService, body: Value) -> Value {
    resolve_revision(service, body, true)
}

fn resolve_revision(service: &TradeAssemblyService, body: Value, persist_revision: bool) -> Value {
    let input = match graph_input(service, &body) {
        Ok(input) => input,
        Err(error) => return error,
    };
    let graph = graph_resolution(service, &input.request);
    let input_value = json!({
        "configId": input.config_id,
        "strategyId": input.strategy_id,
        "strategyVersionId": input.strategy_version_id,
        "strategySpecHash": input.strategy_spec_hash,
        "mode": input.mode,
        "request": input.request,
        "graphFingerprint": graph.graph_fingerprint,
    });
    let input_hash = hash_value(&input_value);
    let idempotency_key = string_field(&body, &["idempotencyKey", "idempotency_key"])
        .unwrap_or_else(|| format!("capability.graph.save:{input_hash}"));
    let idempotency_ref = hash_string(&idempotency_key);

    if let Ok(Some(existing)) = service
        .runtime()
        .storage
        .get_json(IDEMPOTENCY_NS, &idempotency_ref)
    {
        if existing["inputHash"].as_str() != Some(&input_hash) {
            return blocked(
                "idempotency_conflict",
                "Capability graph idempotency key was reused with different input.",
            );
        }
        if let Some(revision_id) = existing["revisionId"].as_str() {
            if let Ok(Some(mut revision)) = service
                .runtime()
                .storage
                .get_json(REVISIONS_NS, revision_id)
            {
                revision["duplicate"] = json!(true);
                return json!({"ok": true, "duplicate": true, "revision": revision});
            }
        }
    }

    let revision_id = format!("caprev_{}", &hash_string(&input_hash)[..24]);
    let created_at = input.request.evaluation_epoch.clone();
    let revision = json!({
        "schemaVersion": REVISION_SCHEMA_VERSION,
        "kind": "StrategyExecutionConfigRevision",
        "revisionId": revision_id,
        "configId": input.config_id,
        "strategyId": input.strategy_id,
        "strategyVersionId": input.strategy_version_id,
        "strategySpecHash": input.strategy_spec_hash,
        "mode": input.mode,
        "graphRequest": input.request,
        "graph": graph,
        "graphFingerprint": graph.graph_fingerprint,
        "selectedManifestFingerprints": selected_manifest_fingerprints(&graph),
        "inputHash": input_hash,
        "idempotencyKey": idempotency_key,
        "authority": authority_value(&body),
        "createdAt": created_at,
        "immutable": true,
        "duplicate": false,
        "noAdvice": true,
    });
    let context = side_effect_context(&body, &idempotency_key);
    let idempotency_record = json!({
        "inputHash": input_hash,
        "revisionId": revision_id,
    });
    if persist_revision {
        if service
            .runtime()
            .storage
            .put_json(REVISIONS_NS, &revision_id, revision.clone(), &context)
            .is_err()
        {
            return blocked(
                "capability_revision_persistence_failed",
                "Capability graph revision could not be persisted.",
            );
        }
        if service
            .runtime()
            .storage
            .put_json(
                IDEMPOTENCY_NS,
                &idempotency_ref,
                idempotency_record,
                &context,
            )
            .is_err()
        {
            return blocked(
                "capability_revision_persistence_failed",
                "Capability graph revision idempotency record could not be persisted.",
            );
        }
    }
    json!({"ok": true, "duplicate": false, "revision": revision})
}

pub(crate) fn get_revision(service: &TradeAssemblyService, revision_id: &str) -> Value {
    match service
        .runtime()
        .storage
        .get_json(REVISIONS_NS, revision_id)
    {
        Ok(Some(revision)) => json!({"ok": true, "revision": revision}),
        Ok(None) => blocked(
            "capability_revision_not_found",
            "Capability graph revision was not found.",
        ),
        Err(_) => blocked(
            "capability_revision_persistence_failed",
            "Capability graph revision could not be read.",
        ),
    }
}

pub(crate) fn check_current(service: &TradeAssemblyService, body: Value) -> Value {
    let Some(revision_id) = string_field(&body, &["revisionId", "revision_id"])
        .or_else(|| body["revision"]["revisionId"].as_str().map(str::to_string))
    else {
        return blocked(
            "capability_revision_not_found",
            "Capability graph revision id is required.",
        );
    };
    let revision = match service
        .runtime()
        .storage
        .get_json(REVISIONS_NS, &revision_id)
    {
        Ok(Some(revision)) => revision,
        Ok(None) => {
            return blocked(
                "capability_revision_not_found",
                "Capability graph revision was not found.",
            )
        }
        Err(_) => {
            return blocked(
                "capability_revision_persistence_failed",
                "Capability graph revision could not be read.",
            )
        }
    };
    let mut request: CapabilityGraphRequest =
        match serde_json::from_value(revision["graphRequest"].clone()) {
            Ok(request) => request,
            Err(_) => {
                return blocked(
                    "capability_revision_mismatch",
                    "Capability graph revision request is invalid.",
                )
            }
        };
    request.evaluation_epoch = evaluation_epoch(&body);
    let fresh = graph_resolution(service, &request);
    let saved: CapabilityGraphResolution = match serde_json::from_value(revision["graph"].clone()) {
        Ok(graph) => graph,
        Err(_) => {
            return blocked(
                "capability_revision_mismatch",
                "Capability graph revision snapshot is invalid.",
            )
        }
    };
    let expected_strategy_hash = current_strategy_hash(
        service,
        revision["strategyId"].as_str().unwrap_or_default(),
        revision["strategyVersionId"].as_str().unwrap_or_default(),
    );
    let strategy_matches = expected_strategy_hash
        .as_deref()
        .is_some_and(|hash| revision["strategySpecHash"].as_str() == Some(hash));
    let selection_matches = selection_signature(&saved) == selection_signature(&fresh);
    let current = saved.ok && fresh.ok && strategy_matches && selection_matches;
    let mut blockers = Vec::new();
    if !strategy_matches {
        blockers.push(json!({
            "code": "strategy_version_mismatch",
            "message": "Strategy version hash no longer matches the saved capability revision.",
            "subject": revision["strategyVersionId"],
        }));
    }
    if !fresh.ok || !selection_matches {
        blockers.push(json!({
            "code": "resolution_stale",
            "message": "Current capability facts do not match the saved graph snapshot.",
            "subject": revision_id,
        }));
    }
    json!({
        "ok": current,
        "current": current,
        "revisionId": revision_id,
        "savedGraphFingerprint": saved.graph_fingerprint,
        "currentGraphFingerprint": fresh.graph_fingerprint,
        "selectionMatches": selection_matches,
        "strategyMatches": strategy_matches,
        "blockers": blockers,
        "graph": fresh,
        "noAdvice": true,
    })
}

pub(crate) fn revision_for_context(
    service: &TradeAssemblyService,
    revision_id: &str,
    strategy_id: &str,
    strategy_version_id: &str,
    mode: &str,
    evaluation_epoch: &str,
) -> Result<Value, Value> {
    let read = get_revision(service, revision_id);
    let Some(revision) = read.get("revision").cloned() else {
        return Err(read);
    };
    if revision["strategyId"].as_str() != Some(strategy_id)
        || revision["strategyVersionId"].as_str() != Some(strategy_version_id)
    {
        return Err(blocked(
            "capability_revision_mismatch",
            "Capability graph revision does not match the requested strategy version.",
        ));
    }
    if revision["mode"].as_str() != Some(mode) {
        return Err(blocked(
            "execution_mode_mismatch",
            "Capability graph revision does not match the requested mode.",
        ));
    }
    if !revision["graph"]["ok"].as_bool().unwrap_or(false) {
        return Err(json!({
            "ok": false,
            "state": "blocked",
            "revisionId": revision_id,
            "blockers": revision["graph"]["blockers"],
            "graph": revision["graph"],
            "noAdvice": true,
        }));
    }
    let currentness = check_current(
        service,
        json!({"revisionId": revision_id, "evaluationEpoch": evaluation_epoch}),
    );
    if !currentness["current"].as_bool().unwrap_or(false) {
        return Err(currentness);
    }
    Ok(revision)
}

pub(crate) fn save_backtest_revision(
    service: &TradeAssemblyService,
    body: &Value,
    version: &Value,
    dataset: &Value,
) -> Result<Value, Value> {
    let strategy_id = string_field(body, &["strategyId", "strategy_id"])
        .unwrap_or_else(|| "strat_local_btc_demo".to_string());
    let version_id = version["id"].as_str().unwrap_or_default();
    let spec_hash = version["specHash"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| hash_value(&version["spec"]));
    let dataset_id = dataset["datasetId"].as_str().unwrap_or("dataset");
    let requirement_id = format!("backtest.dataset:{strategy_id}:{dataset_id}");
    let snapshot_binding =
        exact_plugin_binding(service, "core-runtime", "dataset.snapshot.read_v1", None)?;
    let mut bindings = configured_bindings(body)?;
    let (_, _, mut expected_requirements) = requirements_from_strategy(
        service,
        &json!({"strategyVersionId": version_id}),
        &strategy_id,
        "backtest",
    )?;
    expected_requirements.extend(parse_requirements(
        &[backtest_dataset_requirement(&requirement_id, dataset)],
        "backtest",
        &strategy_id,
        "required",
    ));
    normalize_requirements(&mut expected_requirements);
    add_strategy_defaults(
        service,
        &strategy_id,
        version_id,
        "backtest",
        Some(&snapshot_binding),
        &mut bindings,
    )?;
    bindings.insert(requirement_id.clone(), snapshot_binding.clone());

    let revision = supplied_revision(body).map_or_else(
        || {
            let result = save_revision(
                service,
                json!({
                    "configId": format!("backtest:{dataset_id}"),
                    "strategyId": strategy_id,
                    "strategyVersionId": version_id,
                    "strategySpecHash": spec_hash,
                    "mode": "backtest",
                    "evaluationEpoch": string_field(body, &["evaluationEpoch", "evaluation_epoch"])
                        .unwrap_or_else(|| dataset["createdAt"].as_str().unwrap_or("2026-07-08T00:00:00Z").to_string()),
                    "bindings": bindings,
                    "additionalRequirements": [backtest_dataset_requirement(&requirement_id, dataset)],
                    "authority": body.get("authorityContext").cloned().unwrap_or(Value::Null),
                }),
            );
            result
                .get("revision")
                .cloned()
                .ok_or(result)
        },
        |revision_id| {
            revision_for_context(
                service,
                &revision_id,
                &strategy_id,
                version_id,
                "backtest",
                &evaluation_epoch(body),
            )
        },
    )?;
    require_ready_revision(&revision)?;
    require_complete_requirements(&revision, &expected_requirements)?;
    require_selected_binding(&revision, &requirement_id, &snapshot_binding)?;
    Ok(revision)
}

pub(crate) fn save_execution_revision(
    service: &TradeAssemblyService,
    body: &Value,
    config: &Value,
) -> Result<Value, Value> {
    execution_revision(service, body, config, true)
}

pub(crate) fn prepare_execution_revision(
    service: &TradeAssemblyService,
    body: &Value,
    config: &Value,
) -> Result<Value, Value> {
    execution_revision(service, body, config, false)
}

fn execution_revision(
    service: &TradeAssemblyService,
    body: &Value,
    config: &Value,
    persist_revision: bool,
) -> Result<Value, Value> {
    let strategy_id = config["strategyId"].as_str().unwrap_or_default();
    let version_id = config["strategyVersionId"].as_str().unwrap_or_default();
    let mode = config["mode"].as_str().unwrap_or("paper");
    let provider_ref = config["providerRef"].as_str().unwrap_or_default();
    let data_provider_ref = config["dataProviderRef"].as_str().unwrap_or(provider_ref);
    let account_ref = config["accountRef"].as_str().map(str::to_string);
    let broker_binding = exact_plugin_binding(
        service,
        provider_ref,
        if mode.eq_ignore_ascii_case("live") {
            "broker.live_order_submit"
        } else {
            "broker.paper_order_submit"
        },
        account_ref,
    )?;
    let mut bindings = configured_bindings(body)?;
    let (_, _, mut expected_requirements) = requirements_from_strategy(
        service,
        &json!({"strategyVersionId": version_id}),
        strategy_id,
        mode,
    )?;
    let needs_default_data_binding = expected_requirements.iter().any(|requirement| {
        requirement.capability == "market_data.bars.read@1"
            && !bindings.contains_key(&requirement.requirement_id)
    });
    let mut additional_requirements = vec![execution_broker_requirement(config)];
    if mode.eq_ignore_ascii_case("live") {
        additional_requirements.push(execution_quote_requirement(config));
    }
    expected_requirements.extend(parse_requirements(
        &additional_requirements,
        mode,
        strategy_id,
        "required",
    ));
    normalize_requirements(&mut expected_requirements);
    let data_binding = if needs_default_data_binding {
        Some(exact_plugin_binding(
            service,
            data_provider_ref,
            "marketdata.bars.read_v1",
            config["dataAccountRef"].as_str().map(str::to_string),
        )?)
    } else {
        None
    };
    add_strategy_defaults(
        service,
        strategy_id,
        version_id,
        mode,
        data_binding.as_ref(),
        &mut bindings,
    )?;
    bindings.insert(
        "execution.broker.submit".to_string(),
        broker_binding.clone(),
    );
    let quote_binding = if mode.eq_ignore_ascii_case("live") {
        let binding = exact_plugin_binding(
            service,
            data_provider_ref,
            "marketdata.quote.read",
            config["dataAccountRef"].as_str().map(str::to_string),
        )?;
        bindings.insert("execution.risk.quote".to_string(), binding.clone());
        Some(binding)
    } else {
        None
    };
    let revision = if let Some(revision_id) = supplied_revision(body) {
        let revision = revision_for_context(
            service,
            &revision_id,
            strategy_id,
            version_id,
            mode,
            &evaluation_epoch(body),
        )?;
        require_ready_revision(&revision)?;
        require_complete_requirements(&revision, &expected_requirements)?;
        for (requirement_id, binding) in &bindings {
            require_requested_binding(&revision, requirement_id, binding)?;
        }
        revision
    } else {
        let result = resolve_revision(
            service,
            json!({
                "configId": config["configId"],
                "strategyId": strategy_id,
                "strategyVersionId": version_id,
                "strategySpecHash": config["strategySpecHash"],
                "mode": mode,
                "evaluationEpoch": string_field(body, &["evaluationEpoch", "evaluation_epoch"])
                    .unwrap_or_else(|| "2026-07-08T00:00:00Z".to_string()),
                "bindings": bindings,
                "additionalRequirements": additional_requirements,
                "authority": body.get("authorityContext").cloned().unwrap_or(Value::Null),
            }),
            persist_revision,
        );
        result.get("revision").cloned().ok_or(result)?
    };
    require_requested_binding(&revision, "execution.broker.submit", &broker_binding)?;
    if revision["graph"]["ok"].as_bool().unwrap_or(false) {
        require_selected_binding(&revision, "execution.broker.submit", &broker_binding)?;
        if let Some(quote_binding) = &quote_binding {
            require_requested_binding(&revision, "execution.risk.quote", quote_binding)?;
            require_selected_binding(&revision, "execution.risk.quote", quote_binding)?;
        }
    }
    Ok(revision)
}

fn configured_bindings(body: &Value) -> Result<BTreeMap<String, CapabilityBinding>, Value> {
    parse_bindings(
        body.get("capabilityBindings")
            .or_else(|| body.get("capability_bindings"))
            .or_else(|| body.get("bindings")),
    )
}

fn add_strategy_defaults(
    service: &TradeAssemblyService,
    strategy_id: &str,
    version_id: &str,
    mode: &str,
    data_binding: Option<&CapabilityBinding>,
    bindings: &mut BTreeMap<String, CapabilityBinding>,
) -> Result<(), Value> {
    let body = json!({"strategyVersionId": version_id});
    let (_, _, requirements) = requirements_from_strategy(service, &body, strategy_id, mode)?;
    for requirement in requirements {
        if bindings.contains_key(&requirement.requirement_id) {
            continue;
        }
        let binding = match requirement.capability.as_str() {
            "market_data.bars.read@1" => data_binding.cloned(),
            capability => intrinsic_binding(capability),
        };
        if let Some(binding) = binding {
            bindings.insert(requirement.requirement_id, binding);
        }
    }
    Ok(())
}

fn intrinsic_binding(capability: &str) -> Option<CapabilityBinding> {
    let operation_id = match capability {
        "expression.cel.evaluate@1" => "expression.cel.evaluate_v1",
        "calendar.session.resolve@1" => "calendar.session.resolve_v1",
        "order.intent.build@1" => "order.intent.build_v1",
        "order.option.submit@1" => "order.option.submit_v1",
        "order.option_combo.submit@1" => "order.option_combo.submit_v1",
        "indicator.bars.normalize@1" => "indicator.bars.normalize_v1",
        "indicator.stateful.calculate@1" => "indicator.stateful.calculate_v1",
        other => crate::capability::PORTFOLIO_RESEARCH_OPERATIONS
            .iter()
            .find(|(capability, _)| *capability == other)
            .map(|(_, operation)| *operation)?,
    };
    Some(CapabilityBinding {
        plugin_instance_ref: Some("core-runtime".to_string()),
        plugin_ref: Some("tradeassembly.core-runtime".to_string()),
        operation_id: Some(operation_id.to_string()),
        account_ref: None,
    })
}

fn exact_plugin_binding(
    service: &TradeAssemblyService,
    selected_ref: &str,
    operation_id: &str,
    account_ref: Option<String>,
) -> Result<CapabilityBinding, Value> {
    let plugin = providers::plugin_catalog(service)
        .into_iter()
        .find(|plugin| {
            plugin.instance_ref == selected_ref
                || plugin.plugin_ref == selected_ref
                || plugin.provider_ref == selected_ref
        })
        .ok_or_else(|| {
            blocked(
                "explicit_binding_invalid",
                "Configured plugin instance was not found.",
            )
        })?;
    if !plugin
        .operations
        .iter()
        .any(|operation| operation.id == operation_id)
    {
        return Err(blocked(
            "explicit_binding_invalid",
            "Configured plugin operation was not found.",
        ));
    }
    Ok(CapabilityBinding {
        plugin_instance_ref: Some(plugin.instance_ref),
        plugin_ref: Some(plugin.plugin_ref),
        operation_id: Some(operation_id.to_string()),
        account_ref,
    })
}

fn backtest_dataset_requirement(requirement_id: &str, dataset: &Value) -> Value {
    json!({
        "requirementId": requirement_id,
        "capability": "market_data.bars.read@1",
        "purpose": "Read the selected immutable backtest dataset snapshot.",
        "requiredFor": ["backtest"],
        "stageRefs": ["inputs"],
        "substepRefs": ["load_bars"],
        "constraints": {
            "instrumentFamilies": [instrument_family(dataset["symbol"].as_str().unwrap_or_default())],
            "dataShapes": ["bars"],
            "operations": ["read"],
            "fields": ["open", "high", "low", "close", "volume"],
            "inputPaths": ["market_data.source_bars"],
            "schemaRefs": ["schema://market-data/bars@1"],
            "deterministic": true,
            "replayable": true
        },
        "fallbackPolicy": "none",
        "policyTags": ["immutable_dataset"]
    })
}

fn execution_broker_requirement(config: &Value) -> Value {
    let mode = config["mode"].as_str().unwrap_or("paper");
    json!({
        "requirementId": "execution.broker.submit",
        "capability": if mode.eq_ignore_ascii_case("live") {
            "broker.order_submit.live"
        } else {
            "broker.order_submit.paper"
        },
        "purpose": if mode.eq_ignore_ascii_case("live") {
            "live_order_submission"
        } else {
            "paper_trading"
        },
        "requiredFor": [mode],
        "constraints": {
            "instrumentFamilies": [instrument_family(config["symbol"].as_str().unwrap_or_default())],
            "dataShapes": ["order_intent"],
            "schemaRefs": ["schema://strategy/order-intent@1"]
        },
        "fallbackPolicy": "none",
        "policyTags": ["protected_execution"]
    })
}

fn execution_quote_requirement(config: &Value) -> Value {
    json!({
        "requirementId": "execution.risk.quote",
        "capability": "marketdata.quote",
        "purpose": "live_order_submission",
        "requiredFor": ["live"],
        "constraints": {
            "instrumentFamilies": [instrument_family(config["symbol"].as_str().unwrap_or_default())]
        },
        "fallbackPolicy": "none",
        "policyTags": ["protected_execution"]
    })
}

fn instrument_family(symbol: &str) -> &'static str {
    if symbol.contains('/') || symbol.ends_with("-USD") {
        "crypto_spot"
    } else {
        "equity"
    }
}

fn supplied_revision(body: &Value) -> Option<String> {
    string_field(
        body,
        &[
            "capabilityGraphRevisionId",
            "capability_graph_revision_id",
            "capabilityRevisionId",
            "capability_revision_id",
        ],
    )
}

fn require_ready_revision(revision: &Value) -> Result<(), Value> {
    if revision["graph"]["ok"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        Err(blocked(
            "capability_graph_blocked",
            "Capability graph revision is not ready for the requested operation.",
        ))
    }
}

pub(crate) fn require_complete_requirements(
    revision: &Value,
    expected: &[CapabilityRequirement],
) -> Result<(), Value> {
    let Some(requirements) = revision["graphRequest"]["requirements"].as_array() else {
        return Err(blocked(
            "capability_revision_mismatch",
            "Capability graph revision does not record the complete consumer requirement set.",
        ));
    };
    let mut recorded = requirements
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<Vec<CapabilityRequirement>, _>>()
        .map_err(|_| {
            blocked(
                "capability_revision_mismatch",
                "Capability graph revision contains an invalid consumer requirement set.",
            )
        })?;
    let mut expected = expected.to_vec();
    normalize_requirements(&mut recorded);
    normalize_requirements(&mut expected);
    if recorded == expected {
        Ok(())
    } else {
        Err(blocked(
            "capability_revision_mismatch",
            "Capability graph revision does not match the complete consumer requirement set.",
        ))
    }
}

fn require_selected_binding(
    revision: &Value,
    requirement_id: &str,
    expected: &CapabilityBinding,
) -> Result<(), Value> {
    let selected = revision["graph"]["nodes"]
        .as_array()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|node| node["nodeId"].as_str() == Some(requirement_id))
        })
        .and_then(|node| node.get("selected"));
    let matches = selected.is_some_and(|selected| {
        selected["pluginInstanceRef"].as_str() == expected.plugin_instance_ref.as_deref()
            && selected["pluginRef"].as_str() == expected.plugin_ref.as_deref()
            && selected["operationId"].as_str() == expected.operation_id.as_deref()
            && selected["accountRef"].as_str() == expected.account_ref.as_deref()
    });
    if matches {
        Ok(())
    } else {
        Err(blocked(
            "capability_revision_mismatch",
            "Capability graph revision does not contain the configured binding.",
        ))
    }
}

pub(crate) fn require_requested_binding(
    revision: &Value,
    requirement_id: &str,
    expected: &CapabilityBinding,
) -> Result<(), Value> {
    let binding = revision["graphRequest"]["bindings"].get(requirement_id);
    let matches = binding.is_some_and(|binding| {
        binding["pluginInstanceRef"].as_str() == expected.plugin_instance_ref.as_deref()
            && binding["pluginRef"].as_str() == expected.plugin_ref.as_deref()
            && binding["operationId"].as_str() == expected.operation_id.as_deref()
            && binding["accountRef"].as_str() == expected.account_ref.as_deref()
    });
    if matches {
        Ok(())
    } else {
        Err(blocked(
            "capability_revision_mismatch",
            "Capability graph revision does not record the configured binding.",
        ))
    }
}

struct GraphInput {
    config_id: String,
    strategy_id: String,
    strategy_version_id: String,
    strategy_spec_hash: String,
    mode: String,
    request: CapabilityGraphRequest,
}

fn graph_input(service: &TradeAssemblyService, body: &Value) -> Result<GraphInput, Value> {
    let strategy_id = string_field(body, &["strategyId", "strategy_id"])
        .unwrap_or_else(|| "strat_local_btc_demo".to_string());
    let mode = string_field(body, &["mode", "accountMode", "account_mode"])
        .unwrap_or_else(|| "paper".to_string());
    let (strategy_version_id, strategy_spec_hash, mut request) = if let Some(request) = body
        .get("graphRequest")
        .or_else(|| body.get("graph_request"))
    {
        let mut parsed: CapabilityGraphRequest =
            serde_json::from_value(request.clone()).map_err(|_| {
                blocked(
                    "capability_graph_required",
                    "Capability graph request is invalid.",
                )
            })?;
        if parsed.evaluation_epoch.trim().is_empty() {
            parsed.evaluation_epoch = evaluation_epoch(body);
        }
        (
            string_field(body, &["strategyVersionId", "strategy_version_id"])
                .unwrap_or_else(|| "unbound".to_string()),
            string_field(body, &["strategySpecHash", "strategy_spec_hash"])
                .unwrap_or_else(|| "unbound".to_string()),
            parsed,
        )
    } else if let Some(values) = body.get("requirements").and_then(Value::as_array) {
        (
            string_field(body, &["strategyVersionId", "strategy_version_id"])
                .unwrap_or_else(|| "unbound".to_string()),
            string_field(body, &["strategySpecHash", "strategy_spec_hash"])
                .unwrap_or_else(|| "unbound".to_string()),
            CapabilityGraphRequest {
                requirements: parse_requirements(values, &mode, &strategy_id, "required"),
                bindings: BTreeMap::new(),
                configured_fallbacks: BTreeMap::new(),
                evaluation_epoch: evaluation_epoch(body),
            },
        )
    } else {
        let (version_id, spec_hash, requirements) =
            requirements_from_strategy(service, body, &strategy_id, &mode)?;
        (
            version_id,
            spec_hash,
            CapabilityGraphRequest {
                requirements,
                bindings: BTreeMap::new(),
                configured_fallbacks: BTreeMap::new(),
                evaluation_epoch: evaluation_epoch(body),
            },
        )
    };
    if let Some(values) = body
        .get("additionalRequirements")
        .or_else(|| body.get("additional_requirements"))
        .and_then(Value::as_array)
    {
        request
            .requirements
            .extend(parse_requirements(values, &mode, &strategy_id, "required"));
    }
    if request.requirements.is_empty() {
        return Err(blocked(
            "capability_graph_required",
            "Capability graph requires at least one requirement.",
        ));
    }
    normalize_requirements(&mut request.requirements);
    if body.get("bindings").is_some() {
        request.bindings = parse_bindings(body.get("bindings"))?;
    }
    let top_level_fallbacks = body
        .get("configuredFallbacks")
        .or_else(|| body.get("configured_fallbacks"));
    if top_level_fallbacks.is_some() {
        request.configured_fallbacks = parse_fallbacks(top_level_fallbacks)?;
    }
    if let Some(epoch) = string_field(body, &["evaluationEpoch", "evaluation_epoch"]) {
        request.evaluation_epoch = epoch;
    }
    let config_id = string_field(body, &["configId", "config_id"]).unwrap_or_else(|| {
        format!(
            "cfg_{}",
            &hash_string(&format!("{strategy_id}:{strategy_version_id}:{mode}"))[..16]
        )
    });
    Ok(GraphInput {
        config_id,
        strategy_id,
        strategy_version_id,
        strategy_spec_hash,
        mode,
        request,
    })
}

fn requirements_from_strategy(
    service: &TradeAssemblyService,
    body: &Value,
    strategy_id: &str,
    mode: &str,
) -> Result<(String, String, Vec<CapabilityRequirement>), Value> {
    let requested_version = string_field(body, &["strategyVersionId", "strategy_version_id"]);
    let versions = service.strategy_versions(strategy_id);
    let version = requested_version.as_ref().map_or_else(
        || versions.last().cloned(),
        |requested| {
            versions
                .iter()
                .find(|version| version["id"].as_str() == Some(requested))
                .cloned()
        },
    );
    let Some(version) = version else {
        return Err(blocked(
            "strategy_version_mismatch",
            "Published strategy version was not found.",
        ));
    };
    let version_id = version["id"]
        .as_str()
        .unwrap_or("version-unset")
        .to_string();
    let spec_value = version["spec"].clone();
    let spec = StrategySpec::model_validate(spec_value.clone()).map_err(|_| {
        blocked(
            "capability_graph_required",
            "Published StrategySpec could not be normalized for capability resolution.",
        )
    })?;
    let mut requirements = Vec::new();
    for requirement in spec.capability_requirements.required {
        if !requirement_applies(&requirement.required_for, mode) {
            continue;
        }
        let mut value = serde_json::to_value(requirement).unwrap_or(Value::Null);
        value["declaration"] = json!("required");
        value["mode"] = json!(mode);
        value["strategyId"] = json!(strategy_id);
        value["strategyVersionId"] = json!(version_id);
        requirements.push(providers::requirement_from_body(&value));
    }
    for requirement in spec.capability_requirements.optional {
        if !requirement_applies(&requirement.required_for, mode) {
            continue;
        }
        let mut value = serde_json::to_value(requirement).unwrap_or(Value::Null);
        value["declaration"] = json!("optional");
        value["mode"] = json!(mode);
        value["strategyId"] = json!(strategy_id);
        value["strategyVersionId"] = json!(version_id);
        requirements.push(providers::requirement_from_body(&value));
    }
    let spec_hash = version["specHash"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| hash_value(&spec_value));
    Ok((version_id, spec_hash, requirements))
}

fn requirement_applies(required_for: &[crate::spec::ReadinessLevel], mode: &str) -> bool {
    required_for.is_empty()
        || required_for.iter().any(|level| {
            serde_json::to_value(level)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .as_deref()
                == Some(mode)
        })
}

fn parse_requirements(
    values: &[Value],
    mode: &str,
    strategy_id: &str,
    declaration: &str,
) -> Vec<CapabilityRequirement> {
    values
        .iter()
        .map(|value| {
            let mut value = value.clone();
            if value.get("mode").is_none() {
                value["mode"] = json!(mode);
            }
            if value.get("strategyId").is_none() && value.get("strategy_id").is_none() {
                value["strategyId"] = json!(strategy_id);
            }
            if value.get("declaration").is_none() {
                value["declaration"] = json!(declaration);
            }
            providers::requirement_from_body(&value)
        })
        .collect()
}

fn parse_bindings(value: Option<&Value>) -> Result<BTreeMap<String, CapabilityBinding>, Value> {
    value.map_or_else(
        || Ok(BTreeMap::new()),
        |value| {
            serde_json::from_value(value.clone()).map_err(|_| {
                blocked(
                    "explicit_binding_invalid",
                    "Capability bindings are invalid.",
                )
            })
        },
    )
}

fn parse_fallbacks(
    value: Option<&Value>,
) -> Result<BTreeMap<String, Vec<CapabilityBinding>>, Value> {
    value.map_or_else(
        || Ok(BTreeMap::new()),
        |value| {
            serde_json::from_value(value.clone()).map_err(|_| {
                blocked(
                    "fallback_not_permitted",
                    "Configured capability fallbacks are invalid.",
                )
            })
        },
    )
}

fn normalize_requirements(requirements: &mut [CapabilityRequirement]) {
    for requirement in requirements.iter_mut() {
        requirement.required_for.sort();
        requirement.required_for.dedup();
        requirement.stage_refs.sort();
        requirement.stage_refs.dedup();
        requirement.substep_refs.sort();
        requirement.substep_refs.dedup();
        requirement.dependency_refs.sort();
        requirement.dependency_refs.dedup();
        requirement.policy_tags.sort();
        requirement.policy_tags.dedup();
        requirement.constraints.instrument_families.sort();
        requirement.constraints.instrument_families.dedup();
        requirement.constraints.data_shapes.sort();
        requirement.constraints.data_shapes.dedup();
        requirement.constraints.operations.sort();
        requirement.constraints.operations.dedup();
        requirement.constraints.fields.sort();
        requirement.constraints.fields.dedup();
        requirement.constraints.input_paths.sort();
        requirement.constraints.input_paths.dedup();
        requirement.constraints.schema_refs.sort();
        requirement.constraints.schema_refs.dedup();
    }
    requirements.sort_by(|left, right| left.requirement_id.cmp(&right.requirement_id));
}

fn graph_resolution(
    service: &TradeAssemblyService,
    request: &CapabilityGraphRequest,
) -> CapabilityGraphResolution {
    service.runtime().capability_resolver.resolve_graph(
        request.clone(),
        CapabilityResolverCatalog {
            plugins: providers::plugin_catalog(service),
            entitlements: providers::entitlements(service),
        },
    )
}

fn graph_value(service: &TradeAssemblyService, request: &CapabilityGraphRequest) -> Value {
    serde_json::to_value(graph_resolution(service, request)).unwrap_or_else(|_| {
        blocked(
            "capability_graph_blocked",
            "Capability graph could not be serialized.",
        )
    })
}

fn selected_manifest_fingerprints(graph: &CapabilityGraphResolution) -> Vec<String> {
    let mut fingerprints = graph
        .nodes
        .iter()
        .filter_map(|node| {
            node.selected
                .as_ref()
                .map(|selected| selected.manifest_fingerprint.clone())
        })
        .collect::<Vec<_>>();
    fingerprints.sort();
    fingerprints.dedup();
    fingerprints
}

fn selection_signature(graph: &CapabilityGraphResolution) -> Value {
    json!(graph
        .nodes
        .iter()
        .map(|node| json!({
            "nodeId": node.node_id,
            "required": node.required,
            "selected": node.selected.as_ref().map(|selected| json!({
                "pluginInstanceRef": selected.plugin_instance_ref,
                "pluginRef": selected.plugin_ref,
                "operationId": selected.operation_id,
                "accountRef": selected.account_ref,
                "manifestFingerprint": selected.manifest_fingerprint,
                "credentialRevisionRef": selected.credential_revision_ref,
                "entitlementDecisionRef": selected.entitlement_decision_ref,
            })),
        }))
        .collect::<Vec<_>>())
}

fn current_strategy_hash(
    service: &TradeAssemblyService,
    strategy_id: &str,
    version_id: &str,
) -> Option<String> {
    service
        .strategy_versions(strategy_id)
        .into_iter()
        .find(|version| version["id"].as_str() == Some(version_id))
        .map(|version| {
            version["specHash"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| hash_value(&version["spec"]))
        })
}

fn evaluation_epoch(body: &Value) -> String {
    string_field(
        body,
        &[
            "evaluationEpoch",
            "evaluation_epoch",
            "createdAt",
            "created_at",
        ],
    )
    .or_else(now_rfc3339_second)
    .unwrap_or_else(|| "unspecified".to_string())
}

fn authority_value(body: &Value) -> Value {
    let context = body
        .get("authorityContext")
        .or_else(|| body.get("authority_context"));
    let actor = context
        .and_then(|value| value.get("actor"))
        .and_then(Value::as_str)
        .unwrap_or("local-user");
    let surface = context
        .and_then(|value| value.get("surface"))
        .and_then(Value::as_str)
        .unwrap_or("service");
    let account_mode = string_field(body, &["mode", "accountMode", "account_mode"])
        .or_else(|| {
            context
                .and_then(|value| {
                    value
                        .get("accountMode")
                        .or_else(|| value.get("account_mode"))
                })
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "paper".to_string());
    json!({
        "actor": actor,
        "surface": surface,
        "accountMode": account_mode,
    })
}

fn side_effect_context(body: &Value, idempotency_key: &str) -> SideEffectContext {
    let mut authority = AuthorityContext::local_cli();
    authority.surface = "capability_graph".to_string();
    authority.account_mode = string_field(body, &["mode", "accountMode", "account_mode"])
        .unwrap_or_else(|| "paper".to_string());
    SideEffectContext::new(
        authority,
        IdempotencyKey::new(idempotency_key)
            .unwrap_or_else(|_| IdempotencyKey::new(hash_string(idempotency_key)).expect("hash")),
    )
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn hash_value(value: &Value) -> String {
    hash_string(&serde_json::to_string(value).unwrap_or_default())
}

pub(crate) fn hash_string(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn blocked(code: &str, message: &str) -> Value {
    json!({
        "ok": false,
        "state": "blocked",
        "blockers": [{"code": code, "message": message}],
        "noAdvice": true,
    })
}
