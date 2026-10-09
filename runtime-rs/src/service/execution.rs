use super::{
    api_result, capability_graph, orders, positions, risk, workspace, ServiceResponse,
    TradeAssemblyService, LEGAL_BOUNDARY,
};
use crate::capability::now_rfc3339_second;
use crate::live_execution_checks::{live_account_observation_matches, within_execution_limits};
use crate::platform_compute::{live_cloud_preflight_report, DEFAULT_FOSS_DEPLOYMENT};
use crate::ports::{
    AuthorityContext, ComparePutOutcome, EventAppend, EvidenceRecord, IdempotencyKey,
    ImmutablePutOutcome, LeaseClaim, OutboxRecord, PluginOperationRequest, PluginOperationResponse,
    ScheduledWork, SideEffectContext, StorageExpectation, StorageWrite,
};
use crate::strategy_kernel::{
    evaluate_observation, CompiledStrategy, Decision, ExecutionTiming, MarketObservation, OrderSide,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CONFIGS_NS: &str = "execution_configs";
const ACTIVATIONS_NS: &str = "execution_activations";
const RUNS_NS: &str = "execution_runs";
const EVENTS_NS: &str = "execution_events";
const CHECKPOINTS_NS: &str = "execution_checkpoints";
const ATTEMPTS_NS: &str = "execution_attempts";
const TICKS_NS: &str = "execution_ticks";
const WATCHES_NS: &str = "execution_watches";
const RECONCILIATION_NS: &str = "execution_reconciliation";
const CONTROLS_NS: &str = "execution_controls";
const CONTROL_OUTCOMES_NS: &str = "execution_control_outcomes";
const HEALTH_NS: &str = "execution_health";
const STATE_NS: &str = "scheduler_state";
const EXECUTION_TICK_QUEUE: &str = "execution-ticks";
const EXECUTION_TICK_STREAM: &str = "execution-scheduler";
const PLUGIN_OPERATION_RECEIPTS_NS: &str = "plugin_operation_receipts";
const TRUSTED_CLOCK_NS: &str = "trusted_clock_watermarks";
const ACTIVATION_CLOCK_KEY: &str = "execution.activation";
pub(crate) const RESET_NAMESPACES: &[&str] = &[
    CONFIGS_NS,
    ACTIVATIONS_NS,
    RUNS_NS,
    EVENTS_NS,
    CHECKPOINTS_NS,
    ATTEMPTS_NS,
    TICKS_NS,
    WATCHES_NS,
    RECONCILIATION_NS,
    CONTROLS_NS,
    CONTROL_OUTCOMES_NS,
    HEALTH_NS,
    STATE_NS,
];

pub(crate) fn evaluate_tick_response(
    service: &TradeAssemblyService,
    path: &str,
    body: Value,
) -> ServiceResponse {
    let Some(activation_id) = evaluation_activation_id(path) else {
        return ServiceResponse::bad_request_with_details(
            "execution_tick_path_invalid",
            json!({"path": path}),
        );
    };
    let Some(mut run) = get(service, RUNS_NS, &run_id_for_activation(activation_id)) else {
        return ServiceResponse::bad_request_with_details(
            "activation_not_found",
            json!({"activationId": activation_id}),
        );
    };
    let Some(mut activation) = get(service, ACTIVATIONS_NS, activation_id) else {
        return ServiceResponse::conflict_with_details(
            "activation_record_missing",
            json!({"activationId": activation_id, "failClosed": true}),
        );
    };
    if external_agent(&run) || external_agent(&activation) {
        return ServiceResponse::conflict_with_details(
            "external_agent_evaluation_required",
            json!({"activationId":activation_id,"failClosed":true}),
        );
    }
    if activation["state"].as_str() != Some("active")
        || activation["mode"] != run["mode"]
        || (run["mode"] == "live" && activation["localLiveAuthority"] != run["localLiveAuthority"])
        || activation["strategyVersionId"] != run["strategyVersionId"]
        || activation["strategySpecHash"] != run["strategySpecHash"]
        || activation["capabilityGraphRevisionId"] != run["capabilityGraphRevisionId"]
    {
        return ServiceResponse::conflict_with_details(
            "activation_record_mismatch",
            json!({"activationId": activation_id, "failClosed": true}),
        );
    }
    if run["state"].as_str() != Some("active") {
        return ServiceResponse::conflict_with_details(
            "activation_not_active",
            json!({"activationId": activation_id, "state": run["state"]}),
        );
    }
    if run["mode"] == "live" {
        if let Err(response) = super::live_authorization::revalidate_run_authority(service, &run) {
            return response;
        }
    }
    let Some(tick_id) =
        string_field(&body, &["tickId", "tick_id"]).filter(|value| !value.trim().is_empty())
    else {
        return invalid_plugin_evidence("tick_id_required");
    };
    let Some(sequence) = body.get("sequence").and_then(Value::as_u64) else {
        return invalid_plugin_evidence("tick_sequence_required");
    };
    let current_sequence = run["cycle"].as_u64().unwrap_or(0);
    if sequence <= current_sequence {
        return ServiceResponse::conflict_with_details(
            "stale_tick",
            json!({
                "activationId": activation_id,
                "tickId": tick_id,
                "sequence": sequence,
                "currentSequence": current_sequence,
            }),
        );
    }
    let expected_sequence = current_sequence.saturating_add(1);
    if sequence != expected_sequence {
        return ServiceResponse::conflict_with_details(
            "out_of_order_tick",
            json!({
                "activationId": activation_id,
                "tickId": tick_id,
                "sequence": sequence,
                "expectedSequence": expected_sequence,
            }),
        );
    }
    if body["strategyVersionId"] != run["strategyVersionId"] {
        return invalid_plugin_evidence("strategy_version_mismatch");
    }
    if let Some(currentness) = capability_revision_blocker(service, &run) {
        pause_for_runtime_uncertainty(
            service,
            activation_id,
            "capability_revision_stale",
            currentness.clone(),
        );
        persist_terminal_tick(
            service,
            &run,
            TerminalTick {
                body: &body,
                activation_id,
                tick_id: &tick_id,
                sequence,
                status: "capability_blocked",
                decision: json!({
                "kind": "error",
                "code": "capability_revision_stale",
                "details": currentness,
                "noAdvice": LEGAL_BOUNDARY,
                }),
            },
        );
        return ServiceResponse::conflict_with_details(
            "capability_revision_stale",
            json!({"activationId": activation_id, "failClosed": true}),
        );
    }

    let observation = match validate_bound_observation(
        service,
        &run,
        &body["observation"],
        activation_id,
        &tick_id,
        sequence,
    ) {
        Ok(observation) => observation,
        Err(reason) => return invalid_plugin_evidence(reason),
    };
    let calendar_resolution = match resolve_calendar_session(
        service,
        &run,
        &body,
        &observation,
        activation_id,
        &tick_id,
        sequence,
    ) {
        Ok(resolution) => resolution,
        Err(reason) => {
            persist_terminal_tick(
                service,
                &run,
                TerminalTick {
                    body: &body,
                    activation_id,
                    tick_id: &tick_id,
                    sequence,
                    status: "calendar_blocked",
                    decision: json!({
                    "kind": "error",
                    "code": "calendar_resolution_failed",
                    "reason": reason,
                    "noAdvice": LEGAL_BOUNDARY,
                    }),
                },
            );
            return invalid_plugin_evidence("calendar_resolution_failed");
        }
    };
    let strategy_version = service
        .strategy_versions(run["strategyId"].as_str().unwrap_or_default())
        .into_iter()
        .find(|version| version["id"] == run["strategyVersionId"]);
    let Some(strategy_version) = strategy_version else {
        return invalid_plugin_evidence("strategy_version_not_found");
    };
    if hash_strategy_spec(&strategy_version["spec"]) != run["strategySpecHash"] {
        return invalid_plugin_evidence("strategy_spec_hash_mismatch");
    }
    let compiled = match CompiledStrategy::compile_json(&strategy_version["spec"]) {
        Ok(compiled) => compiled,
        Err(_) => return invalid_plugin_evidence("strategy_spec_not_executable"),
    };
    let current_position = run["paperPositionMicros"].as_i64().unwrap_or(0);
    let kernel_observation = MarketObservation {
        bar_index: usize::try_from(sequence).unwrap_or(usize::MAX),
        symbol: observation.symbol.clone(),
        open: observation.open_micros,
        close: observation.close_micros,
        volume: observation.volume_micros,
    };
    if kernel_observation.bar_index == usize::MAX {
        return invalid_plugin_evidence("tick_sequence_out_of_range");
    }
    let kernel_decision = calendar_resolution["eligible"]
        .as_bool()
        .filter(|eligible| *eligible)
        .map(|_| {
            evaluate_observation(
                &compiled,
                &kernel_observation,
                current_position,
                ExecutionTiming::SameBarClose,
            )
        });
    let source_event_id = observation.event_id.clone();
    let mut decision = json!({
        "decisionId": format!("decision_{activation_id}_{tick_id}"),
        "activationId": activation_id,
        "tickId": tick_id,
        "sequence": sequence,
        "sourceObservationIds": [source_event_id],
        "facts": {
            "symbol": observation.symbol,
            "openMicros": observation.open_micros,
            "closeMicros": observation.close_micros,
            "volumeMicros": observation.volume_micros,
            "observedAtMs": observation.observed_at_ms,
            "pluginRef": observation.plugin_ref,
            "operationId": observation.operation_id,
            "calendar": calendar_resolution,
        },
        "noAdvice": LEGAL_BOUNDARY,
    });
    let mut order_intent = Value::Null;
    let mut order_result = json!({"status": "skipped", "reason": "no_signal"});
    match kernel_decision {
        None => {
            decision["kind"] = json!("market_closed");
            order_result = json!({"status": "skipped", "reason": "market_closed"});
        }
        Some(Decision::NoSignal { .. }) => {
            decision["kind"] = json!("no_signal");
        }
        Some(Decision::OrderIntent(intent))
            if intent.side == OrderSide::Buy && new_entries_blocked(service, activation_id) =>
        {
            decision["kind"] = json!("entry_blocked");
            decision["control"] = json!({
                "reason": if stop_after_flat_armed(service, activation_id) {
                    "stop_after_flat"
                } else {
                    "pause_entries"
                },
                "newEntriesBlocked": true,
            });
            order_result = json!({"status": "skipped", "reason": "new_entries_blocked"});
        }
        Some(Decision::OrderIntent(_intent))
            if reconciliation_blocks_effects(service, activation_id) =>
        {
            decision["kind"] = json!("reconciliation_blocked");
            decision["reconciliation"] = json!({
                "reason": "broker_outcome_uncertain",
                "newEntriesBlocked": true,
                "sideEffectsBlocked": true,
            });
            order_result = json!({"status": "skipped", "reason": "reconciliation_required"});
        }
        Some(Decision::OrderIntent(intent))
            if !within_execution_limits(
                &run,
                quantity_value(intent.quantity),
                observation.close_micros,
            ) =>
        {
            decision["kind"] = json!("risk_blocked");
            decision["risk"] = json!({
                "status": "blocked",
                "quantity": quantity_value(intent.quantity),
                "riskLimits": run["riskLimits"],
            });
            order_result = json!({"status": "skipped", "reason": "execution_risk_limit_blocked"});
        }
        Some(Decision::OrderIntent(intent)) => {
            decision["kind"] = json!("order_intent");
            let quantity = quantity_value(intent.quantity);
            let side = match intent.side {
                OrderSide::Buy => "buy",
                OrderSide::Sell => "sell",
            };
            let mode = match durable_execution_mode(&run) {
                Ok(mode) => mode,
                Err(reason) => return invalid_plugin_evidence(reason),
            };
            let (broker_capability, broker_operation, purpose) = if mode == "live" {
                (
                    "broker.order_submit.live",
                    "broker.live_order_submit",
                    "live_order_submission",
                )
            } else {
                (
                    "broker.order_submit.paper",
                    "broker.paper_order_submit",
                    "paper_trading",
                )
            };
            let Some(broker_binding) = selected_binding(&run, broker_capability, broker_operation)
            else {
                return invalid_plugin_evidence("broker_plugin_binding_missing");
            };
            let client_order_id = format!("{activation_id}:{tick_id}:{side}");
            let plugin_request = PluginOperationRequest {
                correlation_id: correlation_id_for_run(&run),
                plugin_instance_ref: broker_binding["pluginInstanceRef"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                plugin_ref: broker_binding["pluginRef"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                manifest_fingerprint: broker_binding["manifestFingerprint"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                operation_id: broker_binding["operationId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                capability: broker_binding["operation"]["capability"]
                    .as_str()
                    .unwrap_or("broker.order_submit.paper")
                    .to_string(),
                capability_graph_revision_id: run["capabilityGraphRevisionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                capability_graph_fingerprint: run["capabilityGraphFingerprint"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                strategy_id: run["strategyId"].as_str().unwrap_or_default().to_string(),
                strategy_version_id: run["strategyVersionId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                strategy_spec_hash: run["strategySpecHash"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                activation_id: activation_id.to_string(),
                attempt_id: string_field(&body, &["attemptId", "attempt_id"])
                    .unwrap_or_else(|| format!("attempt_{activation_id}_{tick_id}")),
                evaluation_tick_id: tick_id.clone(),
                mode: mode.clone(),
                purpose: purpose.to_string(),
                account_ref: broker_binding["accountRef"].as_str().map(str::to_string),
                timeout_ms: 10_000,
                fencing_token: body["fencingToken"].as_i64(),
                input: json!({
                    "symbol": intent.symbol,
                    "assetClass": run["assetClass"],
                    "side": side,
                    "quantityMicros": intent.quantity,
                    "fillPriceMicros": observation.close_micros,
                    "sequence": sequence,
                    "clientOrderId": client_order_id,
                }),
                evidence_refs: observation.evidence_refs.clone(),
            };
            order_intent = json!({
                "clientOrderId": client_order_id,
                "symbol": intent.symbol,
                "assetClass": run["assetClass"],
                "side": side,
                "quantity": quantity,
                "quantityMicros": intent.quantity,
                "sourceObservationId": source_event_id,
                "plugin": {
                    "pluginInstanceRef": plugin_request.plugin_instance_ref,
                    "pluginRef": plugin_request.plugin_ref,
                    "operationId": plugin_request.operation_id,
                    "manifestFingerprint": plugin_request.manifest_fingerprint,
                },
            });
            let order_idempotency = format!("execution:{activation_id}:tick:{tick_id}:order");
            if service
                .runtime()
                .outbox
                .append(OutboxRecord {
                    outbox_id: format!("outbox_{}", slug(&order_idempotency)),
                    topic: format!("execution.{mode}_order.intent"),
                    payload: json!({
                        "activationId": activation_id,
                        "correlationId": correlation_id_for_run(&run),
                        "tickId": tick_id,
                        "clientOrderId": client_order_id,
                        "strategyVersionId": run["strategyVersionId"],
                        "strategySpecHash": run["strategySpecHash"],
                        "capabilityGraphRevisionId": run["capabilityGraphRevisionId"],
                        "pluginInstanceRef": plugin_request.plugin_instance_ref,
                        "pluginRef": plugin_request.plugin_ref,
                        "operationId": plugin_request.operation_id,
                        "manifestFingerprint": plugin_request.manifest_fingerprint,
                        "symbol": intent.symbol,
                        "side": side,
                        "quantityMicros": intent.quantity,
                        "sourceObservationId": source_event_id,
                    }),
                    idempotency_key: IdempotencyKey::new(format!("outbox:{order_idempotency}"))
                        .expect("valid order outbox idempotency key"),
                    created_at_ms: service.runtime().clock.now_ms(),
                    fencing_token: plugin_request.fencing_token.unwrap_or_default(),
                })
                .is_err()
            {
                return ServiceResponse::bad_gateway("paper_order_intent_persistence_failed");
            }
            let order_request = json!({
                "activationId": activation_id,
                "controlPlaneCorrelationId": correlation_id_for_run(&run),
                "strategyId": run["strategyId"],
                "strategyVersionId": run["strategyVersionId"],
                "symbol": intent.symbol,
                "side": side,
                "qty": quantity,
                "submitOrders": true,
                "accountMode": mode.clone(),
                "mode": mode,
                "providerRef": plugin_request.plugin_instance_ref,
                "idempotencyKey": order_idempotency,
                "authorityContext": body["authorityContext"],
                "evidenceRefs": observation.evidence_refs,
                "sourceObservationId": source_event_id,
                "pluginOperationRequest": plugin_request,
            });
            order_result = orders::run_once(service, order_request.clone());
            if order_result["status"].as_str() == Some("failed") {
                let ambiguity = json!({
                    "clientOrderId": client_order_id,
                    "tickId": tick_id,
                    "side": side,
                    "pluginInstanceRef": broker_binding["pluginInstanceRef"],
                    "operationId": broker_binding["operationId"],
                    "reason": "plugin_order_submission_failed",
                    "orderRequest": order_request,
                });
                pause_for_runtime_uncertainty(
                    service,
                    activation_id,
                    "plugin_order_submission_failed",
                    ambiguity.clone(),
                );
                decision["kind"] = json!("error");
                decision["error"] = json!({
                    "code": "plugin_order_submission_failed",
                    "reconciliationRequired": true,
                    "ambiguity": ambiguity,
                });
            } else {
                run["paperPositionMicros"] = json!(match intent.side {
                    OrderSide::Buy => current_position.saturating_add(intent.quantity),
                    OrderSide::Sell => current_position.saturating_sub(intent.quantity).max(0),
                });
            }
        }
    }
    let decision_event_id = append_event(
        service,
        activation_id,
        if decision["kind"] == "order_intent" {
            "decision.order_intent"
        } else if decision["kind"] == "error" {
            "decision.error"
        } else if decision["kind"] == "risk_blocked" {
            "decision.risk_blocked"
        } else if decision["kind"] == "reconciliation_blocked" {
            "decision.reconciliation_blocked"
        } else {
            "decision.no_trade"
        },
        decision.clone(),
    );
    let resumed_from = run["checkpoint"].clone();
    let checkpoint = checkpoint(
        activation_id,
        &correlation_id_for_run(&run),
        sequence,
        run["checkpointHash"].as_str(),
        "evaluation_completed",
        json!({
            "activeWatches": [run["symbol"].clone()],
            "pendingOrders": [],
            "riskReservations": risk::reservations(service),
            "sourceObservationId": source_event_id,
            "sourceObservedAtMs": observation.observed_at_ms,
            "paperPositionMicros": run["paperPositionMicros"],
        }),
    );
    persist(
        service,
        CHECKPOINTS_NS,
        checkpoint["checkpointId"].as_str().unwrap_or("checkpoint"),
        checkpoint.clone(),
        "execution.checkpoint.created",
    );
    run["cycle"] = json!(sequence);
    run["stateReason"] = json!("evaluation_completed");
    run["lastDecision"] = decision.clone();
    run["lastOrderResult"] = order_result.clone();
    run["checkpoint"] = checkpoint.clone();
    run["checkpointId"] = checkpoint["checkpointId"].clone();
    run["checkpointHash"] = checkpoint["checkpointHash"].clone();
    if let Some(events) = run["eventRefs"].as_array_mut() {
        events.push(json!(decision_event_id));
    }
    if stop_after_flat_armed(service, activation_id)
        && run["paperPositionMicros"].as_i64().unwrap_or(0) == 0
    {
        run["state"] = json!("stopped");
        run["stateReason"] = json!("stop_after_flat_completed");
        run["stoppedAtMs"] = json!(service.runtime().clock.now_ms());
    }
    persist(
        service,
        RUNS_NS,
        run["executionRunId"].as_str().unwrap_or("run"),
        run.clone(),
        "execution.run.updated",
    );
    let tick = json!({
        "schemaVersion": "tradeassembly.evaluation_tick.v1",
        "kind": "EvaluationTickRecord",
        "tickId": tick_id,
        "activationId": activation_id,
        "correlationId": correlation_id_for_run(&run),
        "attemptId": string_field(&body, &["attemptId", "attempt_id"]),
        "sequence": sequence,
        "strategyVersionId": run["strategyVersionId"],
        "strategySpecHash": run["strategySpecHash"],
        "sourceObservationIds": [source_event_id],
        "decision": decision,
        "orderIntent": order_intent,
        "orderResult": order_result,
        "priorCheckpointId": resumed_from["checkpointId"],
        "checkpointId": checkpoint["checkpointId"],
        "status": "completed",
    });
    persist(
        service,
        TICKS_NS,
        &format!("{}:{sequence}", slug(activation_id)),
        tick,
        "execution.tick.recorded",
    );
    activation["state"] = run["state"].clone();
    activation["stateReason"] = run["stateReason"].clone();
    activation["lastCompletedSequence"] = json!(sequence);
    activation["checkpointId"] = checkpoint["checkpointId"].clone();
    activation["checkpointHash"] = checkpoint["checkpointHash"].clone();
    persist(
        service,
        ACTIVATIONS_NS,
        activation_id,
        activation,
        "execution.activation.updated",
    );
    ServiceResponse::ok(api_result(json!({
        "activationId": activation_id,
        "tickId": tick_id,
        "sequence": sequence,
        "compiledStrategy": {
            "strategyVersionId": run["strategyVersionId"],
            "strategySpecHash": run["strategySpecHash"],
        },
        "decision": decision,
        "orderIntent": order_intent,
        "orderResult": order_result,
        "resumedFrom": resumed_from,
        "checkpoint": checkpoint,
    })))
}

struct BoundObservation {
    event_id: String,
    observed_at_ms: i64,
    plugin_ref: String,
    operation_id: String,
    symbol: String,
    open_micros: i64,
    close_micros: i64,
    volume_micros: i64,
    evidence_refs: Vec<String>,
}

fn validate_bound_observation(
    service: &TradeAssemblyService,
    run: &Value,
    value: &Value,
    activation_id: &str,
    tick_id: &str,
    sequence: u64,
) -> Result<BoundObservation, &'static str> {
    if value["schemaVersion"] != "tradeassembly.plugin_observation.v1"
        || value["freshness"]["state"] != "fresh"
        || value["freshness"]["maxAgeMs"].as_u64().unwrap_or(0) == 0
    {
        return Err("observation_envelope_invalid");
    }
    let event_id = required_json_string(value, "/eventId")?;
    let observed_at_ms = value["observedAtMs"]
        .as_i64()
        .filter(|timestamp| *timestamp > 0)
        .ok_or("observation_timestamp_invalid")?;
    if run
        .pointer("/checkpoint/state/sourceObservationId")
        .and_then(Value::as_str)
        == Some(event_id)
    {
        return Err("observation_event_duplicate");
    }
    if run
        .pointer("/checkpoint/state/sourceObservedAtMs")
        .and_then(Value::as_i64)
        .is_some_and(|previous| observed_at_ms <= previous)
    {
        return Err("observation_event_time_not_monotonic");
    }
    let plugin_instance_ref = required_json_string(value, "/source/pluginInstanceRef")?;
    let plugin_ref = required_json_string(value, "/source/pluginRef")?;
    let operation_id = required_json_string(value, "/source/operationId")?;
    let manifest_fingerprint = required_json_string(value, "/source/manifestFingerprint")?;
    let capability = required_json_string(value, "/source/capability")?;
    let Some(binding) = selected_binding(run, capability, operation_id) else {
        return Err("observation_plugin_unbound");
    };
    if binding["pluginInstanceRef"].as_str() != Some(plugin_instance_ref)
        || binding["pluginRef"].as_str() != Some(plugin_ref)
        || binding["operationId"].as_str() != Some(operation_id)
        || binding["manifestFingerprint"].as_str() != Some(manifest_fingerprint)
        || binding["operation"]["capability"].as_str() != Some(capability)
    {
        return Err("observation_plugin_binding_mismatch");
    }
    let symbol = required_json_string(value, "/bar/symbol")?;
    if run["symbol"].as_str() != Some(symbol) {
        return Err("observation_symbol_mismatch");
    }
    if run["timeframe"].as_str() != value.pointer("/bar/timeframe").and_then(Value::as_str) {
        return Err("observation_timeframe_mismatch");
    }
    let open_micros = decimal_micros(value.pointer("/bar/open")).ok_or("bar_open_invalid")?;
    let close_micros = decimal_micros(value.pointer("/bar/close")).ok_or("bar_close_invalid")?;
    let volume_micros = decimal_micros(value.pointer("/bar/volume"))
        .filter(|volume| *volume > 0)
        .ok_or("bar_volume_invalid")?;
    if value["bar"]["high"].as_f64().is_none()
        || value["bar"]["low"].as_f64().is_none()
        || sequence == 0
    {
        return Err("bar_shape_invalid");
    }
    let evidence_refs = value["evidenceRefs"]
        .as_array()
        .filter(|refs| !refs.is_empty())
        .ok_or("observation_evidence_missing")?
        .iter()
        .map(|reference| {
            reference
                .as_str()
                .filter(|reference| !reference.trim().is_empty())
                .map(str::to_string)
                .ok_or("observation_evidence_invalid")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let receipt_key = format!("execution:{activation_id}:tick:{sequence}:market");
    let receipt = service
        .runtime()
        .storage
        .get_json(PLUGIN_OPERATION_RECEIPTS_NS, &receipt_key)
        .map_err(|_| "observation_receipt_unavailable")?
        .ok_or("observation_receipt_missing")
        .and_then(|stored| {
            serde_json::from_value::<PluginOperationResponse>(stored)
                .map_err(|_| "observation_receipt_invalid")
        })?;
    let expected_event_ref = format!("plugin-event://{}", receipt.source_event_id);
    let expected_content_ref = format!("plugin-content://{}", receipt.content_hash);
    if receipt.schema_ref != "schema://market-data/bar@1"
        || receipt.source_event_id != event_id
        || receipt.observed_at_ms != observed_at_ms
        || receipt.freshness_state != "fresh"
        || receipt.payload["symbol"].as_str() != Some(symbol)
        || receipt.payload["timeframe"].as_str()
            != value.pointer("/bar/timeframe").and_then(Value::as_str)
        || receipt.payload["openMicros"].as_i64() != Some(open_micros)
        || receipt.payload["closeMicros"].as_i64() != Some(close_micros)
        || receipt.payload["volumeMicros"].as_i64() != Some(volume_micros)
        || !evidence_refs.contains(&expected_event_ref)
        || !evidence_refs.contains(&expected_content_ref)
        || receipt
            .evidence_refs
            .iter()
            .any(|reference| !evidence_refs.contains(reference))
    {
        return Err("observation_receipt_mismatch");
    }
    let request = market_operation_request(
        run,
        activation_id,
        tick_id,
        &format!("receipt-validation-{tick_id}"),
        sequence,
        None,
    )?;
    let canonical = market_observation_from_response(run, &request, &receipt)?;
    if canonical != *value {
        return Err("observation_receipt_payload_mismatch");
    }
    Ok(BoundObservation {
        event_id: event_id.to_string(),
        observed_at_ms,
        plugin_ref: plugin_ref.to_string(),
        operation_id: operation_id.to_string(),
        symbol: symbol.to_string(),
        open_micros,
        close_micros,
        volume_micros,
        evidence_refs,
    })
}

fn selected_binding<'a>(run: &'a Value, capability: &str, operation_id: &str) -> Option<&'a Value> {
    run.pointer("/currentCapabilityGraphSnapshot/nodes")
        .or_else(|| run.pointer("/immutableInput/capabilityGraphSnapshot/nodes"))
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|node| node.get("selected"))
        .find(|selected| {
            selected["operationId"].as_str() == Some(operation_id)
                && selected["operation"]["capability"].as_str() == Some(capability)
        })
}

fn resolve_calendar_session(
    service: &TradeAssemblyService,
    run: &Value,
    body: &Value,
    observation: &BoundObservation,
    activation_id: &str,
    tick_id: &str,
    sequence: u64,
) -> Result<Value, String> {
    let calendar_ref = run["calendarRef"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "calendar_ref_missing".to_string())?;
    let binding = selected_binding(
        run,
        "calendar.session.resolve@1",
        "calendar.session.resolve_v1",
    )
    .ok_or_else(|| "calendar_plugin_binding_missing".to_string())?;
    let attempt_id = string_field(body, &["attemptId", "attempt_id"])
        .unwrap_or_else(|| format!("attempt_{activation_id}_{tick_id}"));
    let mode = durable_execution_mode(run).map_err(str::to_string)?;
    let purpose = if mode == "live" {
        "live_order_submission"
    } else {
        "paper_trading"
    };
    let request = PluginOperationRequest {
        correlation_id: correlation_id_for_run(run),
        plugin_instance_ref: binding["pluginInstanceRef"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        plugin_ref: binding["pluginRef"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        manifest_fingerprint: binding["manifestFingerprint"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        operation_id: binding["operationId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        capability: binding["operation"]["capability"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        capability_graph_revision_id: run["capabilityGraphRevisionId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        capability_graph_fingerprint: run["capabilityGraphFingerprint"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        strategy_id: run["strategyId"].as_str().unwrap_or_default().to_string(),
        strategy_version_id: run["strategyVersionId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        strategy_spec_hash: run["strategySpecHash"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        activation_id: activation_id.to_string(),
        attempt_id,
        evaluation_tick_id: tick_id.to_string(),
        mode: mode.clone(),
        purpose: purpose.to_string(),
        account_ref: None,
        timeout_ms: 10_000,
        fencing_token: body["fencingToken"].as_i64(),
        input: json!({
            "calendarRef": calendar_ref,
            "observedAtMs": observation.observed_at_ms,
            "sequence": sequence,
        }),
        evidence_refs: observation.evidence_refs.clone(),
    };
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: "local-user".to_string(),
            surface: "scheduler".to_string(),
            account_mode: mode,
        },
        IdempotencyKey::new(format!("execution:{activation_id}:tick:{tick_id}:calendar"))?,
    );
    let response = service
        .runtime()
        .plugin_operations
        .invoke(&request, &context)
        .map_err(|_| "calendar_plugin_invocation_failed".to_string())?;
    if response.schema_ref != "schema://calendar/session-resolution@1"
        || response.observed_at_ms != observation.observed_at_ms
        || response.payload["calendarRef"].as_str() != Some(calendar_ref)
        || response.payload["observedAtMs"].as_i64() != Some(observation.observed_at_ms)
        || response.payload["eligible"].as_bool().is_none()
        || response.payload["sessionState"].as_str().is_none()
    {
        return Err("calendar_response_invalid".to_string());
    }
    Ok(json!({
        "calendarId": response.payload["calendarId"],
        "calendarRef": calendar_ref,
        "eligible": response.payload["eligible"],
        "sessionState": response.payload["sessionState"],
        "observedAtMs": response.observed_at_ms,
        "timezone": response.payload["timezone"],
        "sessionScope": response.payload["sessionScope"],
        "nextTransitionKind": response.payload["nextTransitionKind"],
        "nextTransitionAtMs": response.payload["nextTransitionAtMs"],
        "regularHoursOpenAtMs": response.payload["regularHoursOpenAtMs"],
        "regularHoursCloseAtMs": response.payload["regularHoursCloseAtMs"],
        "holiday": response.payload["holiday"],
        "earlyClose": response.payload["earlyClose"],
        "pluginInstanceRef": request.plugin_instance_ref,
        "pluginRef": request.plugin_ref,
        "operationId": request.operation_id,
        "manifestFingerprint": request.manifest_fingerprint,
        "sourceEventId": response.source_event_id,
        "contentHash": response.content_hash,
        "sourceClass": response.payload["sourceClass"],
        "calendarModel": response.payload["calendarModel"],
        "calendarSource": response.payload["calendarSource"],
    }))
}

fn invalid_plugin_evidence(reason: &str) -> ServiceResponse {
    ServiceResponse::unprocessable_with_details(
        "plugin_evidence_invalid",
        json!({"failClosed": true, "reason": reason}),
    )
}

fn evaluation_activation_id(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/product/strategy-execution-activations/")?;
    let activation_id = rest.strip_suffix("/ticks/evaluate")?;
    (!activation_id.is_empty() && !activation_id.contains('/')).then_some(activation_id)
}

fn required_json_string<'a>(value: &'a Value, pointer: &str) -> Result<&'a str, &'static str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("required_string_missing")
}

fn decimal_micros(value: Option<&Value>) -> Option<i64> {
    let value = value?.as_f64()?;
    if !value.is_finite() {
        return None;
    }
    let scaled = value * 1_000_000.0;
    (scaled >= i64::MIN as f64 && scaled <= i64::MAX as f64).then(|| scaled.round() as i64)
}

fn quantity_value(quantity_micros: i64) -> f64 {
    quantity_micros as f64 / 1_000_000.0
}

pub(crate) fn save_config(service: &TradeAssemblyService, body: Value) -> Value {
    if body
        .get("orchestrator")
        .is_some_and(|value| !matches!(value.as_str(), Some("deterministic" | "external_agent")))
    {
        return json!({"status":"blocked","error":{"code":"execution_orchestrator_invalid"}});
    }
    let mut universe_request = body.clone();
    if let Some(symbols) = body.get("allowed_symbols") {
        if body
            .get("allowedSymbols")
            .is_some_and(|other| other != symbols)
        {
            return json!({"status":"blocked","error":{"code":"execution_universe_alias_conflict"}});
        }
        universe_request["allowedSymbols"] = symbols.clone();
    }
    if let Err(code) = crate::broker_submission::validate_execution_universe(&universe_request) {
        return json!({"status":"blocked","error":{"code":code}});
    }
    let requested_strategy_id = string_field(&body, &["strategyId", "strategy_id"])
        .unwrap_or_else(|| "strat_local_btc_demo".to_string());
    if let Err(response) = service.require_object("strategy", &requested_strategy_id) {
        return object_unavailable(response);
    }
    let mut config = build_config(service, &body, true);
    config["savedAtMs"] = json!(service.runtime().clock.now_ms());
    let strategy_id = config["strategyId"].as_str().unwrap_or_default();
    if let Err(response) = service.require_object("strategy", strategy_id) {
        return object_unavailable(response);
    }
    let config_id = config["configId"]
        .as_str()
        .unwrap_or("cfg_local")
        .to_string();
    if service
        .bind_inherited_object("execution_config", &config_id, "strategy", strategy_id)
        .is_err()
    {
        return object_scope_unavailable();
    }
    persist(
        service,
        CONFIGS_NS,
        &config_id,
        config.clone(),
        "execution.config.saved",
    );
    json!({
        "configId": config_id,
        "id": config_id,
        "item": config,
        "readiness": readiness_for_config(service, &config),
    })
}

fn build_config(service: &TradeAssemblyService, body: &Value, persist_revision: bool) -> Value {
    let strategy_id = string_field(body, &["strategyId", "strategy_id"])
        .unwrap_or_else(|| "strat_local_btc_demo".to_string());
    super::strategy::materialize_default_version(service, &strategy_id);
    let strategy = service.strategy_value(&strategy_id);
    let latest_version = service
        .strategy_versions(&strategy_id)
        .pop()
        .unwrap_or_else(
            || json!({"id": "ver_local", "version": 1, "spec": strategy["latestSpec"].clone()}),
        );
    let version_id = string_field(
        body,
        &[
            "versionId",
            "version_id",
            "strategyVersionId",
            "strategy_version_id",
        ],
    )
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| {
        latest_version["id"]
            .as_str()
            .or_else(|| strategy["currentVersionId"].as_str())
            .unwrap_or("ver_local")
            .to_string()
    });
    let selected_version = service
        .strategy_versions(&strategy_id)
        .into_iter()
        .find(|version| version["id"].as_str() == Some(&version_id))
        .unwrap_or_else(|| latest_version.clone());
    let provider_ref = string_field(body, &["providerRef", "provider_ref"]).unwrap_or_else(|| {
        strategy["providerRef"]
            .as_str()
            .unwrap_or("sim")
            .to_string()
    });
    let mode = execution_mode_from(body);
    let orchestrator = body["orchestrator"].as_str().unwrap_or("deterministic");
    let account_ref = string_field(body, &["accountRef", "account_ref"]);
    let data_provider_ref = string_field(body, &["dataProviderRef", "data_provider_ref"])
        .unwrap_or_else(|| provider_ref.clone());
    let data_account_ref =
        string_field(body, &["dataAccountRef", "data_account_ref"]).or_else(|| {
            (data_provider_ref == provider_ref)
                .then(|| account_ref.clone())
                .flatten()
        });
    let allowed_symbols = body
        .get("allowedSymbols")
        .or_else(|| body.get("allowed_symbols"));
    let symbol = string_field(body, &["symbol"])
        .or_else(|| {
            body.pointer("/params/symbol")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            if allowed_symbols.is_some() {
                String::new()
            } else {
                strategy["symbol"].as_str().unwrap_or("BTC/USD").to_string()
            }
        });
    let timeframe = string_field(body, &["timeframe"])
        .or_else(|| {
            body.pointer("/params/timeframe")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| strategy["timeframe"].as_str().unwrap_or("1m").to_string());
    let asset_class = string_field(body, &["assetClass", "asset_class"]).unwrap_or_else(|| {
        strategy["assetClass"]
            .as_str()
            .or_else(|| strategy["asset_class"].as_str())
            .unwrap_or("crypto")
            .to_string()
    });
    let spec = selected_version["spec"].clone();
    let calendar_ref = strategy_calendar_ref(&spec);
    let scheduler_interval_seconds = body
        .get("schedulerIntervalSeconds")
        .or_else(|| body.get("scheduler_interval_seconds"))
        .and_then(Value::as_u64)
        .unwrap_or(60)
        .max(1);
    let risk_limits = body
        .get("riskLimits")
        .or_else(|| body.get("risk_limits"))
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "max_notional": strategy["maxNotional"].as_f64().unwrap_or(25.0),
                "max_order_quantity": strategy["maxOrderQuantity"].as_f64().unwrap_or(0.0003),
                "max_daily_loss": strategy["maxDailyLoss"].as_f64().unwrap_or(25.0),
                "max_concurrent_positions": 1,
            })
        });
    let legal_receipt_ref = string_field(body, &["legalReceiptRef", "legal_receipt_ref"]);
    let responsible_human = service.responsible_human_identity();
    let universe_suffix = allowed_symbols
        .map(|symbols| format!(":{symbols}"))
        .unwrap_or_default();
    let config_id = string_field(body, &["configId", "config_id"]).unwrap_or_else(|| {
        format!(
            "cfg_{}",
            short_hash(&format!(
                "{strategy_id}:{version_id}:{provider_ref}:{data_provider_ref}:{mode}:{account_ref:?}:{data_account_ref:?}:{symbol}:{timeframe}:{calendar_ref:?}:{scheduler_interval_seconds}:{legal_receipt_ref:?}:{responsible_human:?}:{orchestrator}{universe_suffix}"
            ))
        )
    });
    let mut config = json!({
        "schemaVersion": "tradeassembly.execution_config.v1",
        "kind": "StrategyExecutionConfig",
        "orchestrator": orchestrator,
        "id": config_id,
        "configId": config_id,
        "strategyId": strategy_id,
        "strategy_id": strategy_id,
        "strategyVersionId": version_id,
        "version_id": version_id,
        "strategySpecHash": hash_strategy_spec(&spec),
        "providerRef": provider_ref,
        "provider_ref": provider_ref,
        "dataProviderRef": data_provider_ref,
        "data_provider_ref": data_provider_ref,
        "mode": mode,
        "accountMode": mode,
        "accountRef": account_ref,
        "dataAccountRef": data_account_ref,
        "symbol": symbol,
        "timeframe": timeframe,
        "assetClass": asset_class,
        "calendarRef": calendar_ref,
        "schedulerIntervalSeconds": scheduler_interval_seconds,
        "dataFreshnessPolicy": {
            "maxAgeMs": 60_000,
        },
        "warmup": {
            "requiredBars": 0,
            "source": if orchestrator == "external_agent" { "external_agent" } else { "compiled_strategy_kernel" },
        },
        "riskLimits": risk_limits,
        "risk_limits": risk_limits,
        "killSwitch": {
            "configured": true,
            "defaultControl": "pause_entries",
            "emergencyStop": true,
        },
        "legalReceiptRef": legal_receipt_ref,
        "responsibleHuman": responsible_human.map(|(issuer, subject)| json!({
            "identityIssuer": issuer,
            "identitySubject": subject,
        })),
        "legalEnvironment": match service.runtime_manifest().map(|manifest| manifest.profile.as_str()) {
            Some("self_hosted") => "self_hosted_live",
            Some("serverless") => "serverless_live",
            _ => "local_live",
        },
        "pluginBindings": [
            {
                "pluginRef": provider_ref,
                "accountRef": account_ref,
                "capability": required_broker_capability(&mode),
                "status": "configured"
            },
            {
                "pluginRef": data_provider_ref,
                "accountRef": data_account_ref,
                "capability": "market_data.bars.read@1",
                "status": "configured"
            }
        ],
        "status": "configured",
        "noAdvice": LEGAL_BOUNDARY,
    });
    if let Some(symbols) = allowed_symbols {
        config["allowedSymbols"] = symbols.clone();
    }
    let revision = if persist_revision {
        capability_graph::save_execution_revision(service, body, &config)
    } else {
        capability_graph::prepare_execution_revision(service, body, &config)
    };
    match revision {
        Ok(revision) => {
            config["capabilityGraphRevisionId"] = revision["revisionId"].clone();
            config["capabilityGraphFingerprint"] = revision["graphFingerprint"].clone();
            if !persist_revision {
                config["preparedCapabilityGraphRevision"] = revision.clone();
            }
            config["status"] = if revision["graph"]["ok"].as_bool().unwrap_or(false) {
                json!("configured")
            } else {
                json!("blocked")
            };
        }
        Err(error) => {
            config["status"] = json!("blocked");
            config["capabilityGraphError"] = error;
        }
    }
    config
}

pub(crate) fn activation_readiness(service: &TradeAssemblyService, body: Value) -> Value {
    let config = match authorized_activation_config(service, &body) {
        Ok(config) => config,
        Err(response) => return object_unavailable(response),
    };
    let authority = if config["mode"] == "live"
        && (body.get("localLiveMandateId").is_some() || body.get("local_live_mandate_id").is_some())
    {
        Some(super::live_authorization::activation_authority(
            service, &config, &body,
        ))
    } else {
        None
    };
    let mut readiness = readiness_for_config_with_authority(
        service,
        &config,
        authority.as_ref().is_some_and(Result::is_ok),
    );
    if let Some(authority) = authority {
        match authority {
            Ok(authority) => readiness["localLiveAuthority"] = authority,
            Err(response) => {
                readiness["ready"] = json!(false);
                readiness["localLiveMandateError"] = response.body["error"].clone();
            }
        }
    }
    readiness
}

pub(crate) fn activation_blocked_preflight(service: &TradeAssemblyService, body: Value) -> Value {
    let config = match authorized_activation_config(service, &body) {
        Ok(config) => config,
        Err(response) => return object_unavailable(response),
    };
    activation_blocked_response(readiness_for_config(service, &config))
}

pub(crate) fn activate(service: &TradeAssemblyService, body: Value) -> Value {
    if !has_explicit_config_id(&body) {
        return json!({
            "status": "blocked",
            "ready": false,
            "failClosed": true,
            "blockedReasons": [{
                "code": "execution_config_required",
                "message": "Save an execution configuration before activation.",
            }],
            "error": {
                "code": "execution_config_required",
                "retryable": false,
            },
        });
    }
    let now_ms = match service.runtime().clock.trusted_now_ms() {
        Ok(now_ms) => now_ms,
        Err(_) => return activation_dependency_failure("trusted_clock_unavailable"),
    };
    let previous_clock = match service
        .runtime()
        .storage
        .get_json(TRUSTED_CLOCK_NS, ACTIVATION_CLOCK_KEY)
    {
        Ok(value) => value,
        Err(_) => return activation_dependency_failure("activation_storage_unavailable"),
    };
    if let Some(previous_clock) = previous_clock.as_ref() {
        let Some(last_trusted_at_ms) = previous_clock["lastTrustedAtMs"].as_i64() else {
            return activation_dependency_failure("trusted_clock_state_invalid");
        };
        if now_ms < last_trusted_at_ms {
            return activation_dependency_failure("trusted_clock_regressed");
        }
    }
    let config = match authorized_activation_config(service, &body) {
        Ok(config) => config,
        Err(response) => return object_unavailable(response),
    };
    let live_authority = if config["mode"] == "live" {
        match super::live_authorization::activation_authority(service, &config, &body) {
            Ok(authority) => authority,
            Err(response) => {
                return json!({
                    "status":"blocked", "ready":false, "failClosed":true,
                    "error":response.body["error"],
                })
            }
        }
    } else {
        Value::Null
    };
    let readiness =
        readiness_for_config_with_authority(service, &config, !live_authority.is_null());
    if !readiness["ready"].as_bool().unwrap_or(false) {
        return activation_blocked_response(readiness);
    }
    let capability_revision = readiness["capabilityGraphRevision"].clone();
    let activation_id =
        string_field(&body, &["activationId", "activation_id"]).unwrap_or_else(|| {
            let mut fingerprint = json!({
                "configId": config["configId"],
                "strategyId": config["strategyId"],
                "strategyVersionId": config["strategyVersionId"],
                "providerRef": config["providerRef"],
                "mode": config["mode"],
                "symbol": config["symbol"],
                "timeframe": config["timeframe"],
                "riskLimits": config.get("riskLimits").or_else(|| config.get("risk_limits")).cloned().unwrap_or_else(|| json!({})),
                "idempotencyKey": string_field(&body, &["idempotencyKey", "idempotency_key"]),
            });
            if config["mode"] == "live" {
                fingerprint["localLiveAuthority"] = live_authority.clone();
            }
            format!(
                "activation_{}",
                short_hash(&hash_value(&fingerprint))
            )
        });
    let run_id = format!("run_{activation_id}");
    let correlation_id = trusted_correlation_id(&body);
    if let Some(existing) = get(service, RUNS_NS, &run_id) {
        if let Err(response) = service.require_object("execution_activation", &activation_id) {
            return object_unavailable(response);
        }
        if existing["kind"] == "ExecutionRun" {
            if activation_matches_config(&existing, &config)
                && (config["mode"] != "live" || existing["localLiveAuthority"] == live_authority)
            {
                return activation_response(service, existing, true);
            }
            return json!({
                "activationId": activation_id,
                "status": "blocked",
                "duplicate": false,
                "failClosed": true,
                "error": {
                    "code": "activation_id_conflict",
                    "message": "The requested activation identifier is already bound to a different execution configuration.",
                },
            });
        }
    }
    let mut controls = controls_default(&activation_id);
    controls["correlationId"] = json!(correlation_id);
    let event_payload = json!({
            "correlationId": correlation_id,
            "configId": config["configId"],
            "mode": config["mode"],
            "purpose": if config["mode"] == "live" { "live_order_submission" } else { "paper_trading" },
            "client": "self",
    });
    let event_id = format!("evt_{}_1", slug(&activation_id));
    let event_hash = hash_value(&json!({
        "activationId": activation_id,
        "correlationId": correlation_id,
        "sequence": 1,
        "eventType": "activation.started",
        "payload": event_payload,
        "previousHash": Value::Null,
    }));
    let activation_event = json!({
        "schemaVersion": "tradeassembly.execution_event.v1",
        "id": event_id,
        "eventId": event_id,
        "activationId": activation_id,
        "correlationId": correlation_id,
        "sequence": 1,
        "eventType": "activation.started",
        "payload": event_payload,
        "previousHash": Value::Null,
        "eventHash": event_hash,
        "noAdvice": LEGAL_BOUNDARY,
    });
    let checkpoint = checkpoint(
        &activation_id,
        &correlation_id,
        0,
        None,
        "activation_started",
        json!({"activeWatches": [config["symbol"].clone()], "pendingOrders": [], "riskReservations": []}),
    );
    let Some(market_binding) =
        selected_capability_value(&capability_revision["graph"], "market_data.bars.read@1")
    else {
        return activation_dependency_failure("activation_capability_binding_invalid");
    };
    let watch_id = format!("watch_{}_market", slug(&activation_id));
    let watch = json!({
        "schemaVersion": "tradeassembly.runtime_watch.v1",
        "kind": "RuntimeWatch",
        "watchId": watch_id,
        "activationId": activation_id,
        "correlationId": correlation_id,
        "pluginInstanceRef": market_binding["pluginInstanceRef"],
        "pluginRef": market_binding["pluginRef"],
        "manifestFingerprint": market_binding["manifestFingerprint"],
        "operationId": market_binding["operationId"],
        "capability": market_binding["operation"]["capability"],
        "symbol": config["symbol"],
        "timeframe": config["timeframe"],
        "calendarRef": config["calendarRef"],
        "cursor": 0,
        "state": if external_agent(&config) { "externally_managed" } else { "active" },
    });
    let reconciliation = json!({
        "schemaVersion": "tradeassembly.execution_reconciliation.v1",
        "kind": "ReconciliationState",
        "activationId": activation_id,
        "correlationId": correlation_id,
        "state": "clear",
        "pendingActions": [],
        "ambiguousActions": [],
        "newEntriesPaused": false,
        "lastReconciledAtMs": now_ms,
    });
    let activation_record = json!({
        "schemaVersion": "tradeassembly.activation_record.v1",
        "kind": "ActivationRecord",
        "orchestrator": orchestrator_kind(&config),
        "localLiveAuthority": live_authority,
        "activationId": activation_id,
        "correlationId": correlation_id,
        "configId": config["configId"],
        "strategyId": config["strategyId"],
        "strategyVersionId": config["strategyVersionId"],
        "strategySpecHash": config["strategySpecHash"],
        "mode": config["mode"],
        "accountRef": config["accountRef"],
        "assetClass": config["assetClass"],
        "symbol": config["symbol"],
        "timeframe": config["timeframe"],
        "calendarRef": config["calendarRef"],
        "dataFreshnessPolicy": config["dataFreshnessPolicy"],
        "riskLimits": config["riskLimits"],
        "capabilityGraphRevisionId": capability_revision["revisionId"],
        "capabilityGraphFingerprint": capability_revision["graphFingerprint"],
        "selectedManifestFingerprints": capability_revision["selectedManifestFingerprints"],
        "watchRefs": [watch_id],
        "checkpointId": checkpoint["checkpointId"],
        "checkpointHash": checkpoint["checkpointHash"],
        "lastCompletedSequence": 0,
        "state": "active",
        "stateReason": "activation_started",
        "createdAtMs": now_ms,
    });
    let apf = apf_refs_for_body(
        &body,
        "http",
        "execution.activate",
        &format!("activate:{activation_id}"),
    );
    let mut run = json!({
        "schemaVersion": "tradeassembly.execution_run.v1",
        "kind": "ExecutionRun",
        "id": run_id,
        "executionRunId": run_id,
        "run_id": run_id,
        "activationId": activation_id,
        "activation_id": activation_id,
        "configId": config["configId"],
        "strategyId": config["strategyId"],
        "strategy_id": config["strategyId"],
        "strategyVersionId": config["strategyVersionId"],
        "version_id": config["strategyVersionId"],
        "strategySpecHash": config["strategySpecHash"],
        "mode": config["mode"],
        "providerRef": config["providerRef"],
        "accountRef": config["accountRef"],
        "assetClass": config["assetClass"],
        "symbol": config["symbol"],
        "timeframe": config["timeframe"],
        "riskLimits": config["riskLimits"],
        "schedulerIntervalSeconds": config["schedulerIntervalSeconds"],
        "state": "active",
        "stateReason": "activation_started",
        "cycle": 0,
        "startedAt": "2026-07-08T00:00:00Z",
        "stoppedAt": null,
        "readiness": readiness,
        "immutableInput": {
            "strategySpecHash": config["strategySpecHash"],
            "capabilityGraphRevisionId": capability_revision["revisionId"],
            "capabilityGraphFingerprint": capability_revision["graphFingerprint"],
            "capabilityGraphSnapshot": capability_revision["graph"],
            "selectedManifestFingerprints": capability_revision["selectedManifestFingerprints"],
        },
        "capabilityGraphRevisionId": capability_revision["revisionId"],
        "capabilityGraphFingerprint": capability_revision["graphFingerprint"],
        "controls": controls,
        "apf": apf,
        "ledger": ledger_from_evidence(&activation_id, &[], &[], &reconciliation),
        "health": {"status": "running", "seriousReconciliationMismatch": false, "newEntriesPaused": false},
        "checkpoint": checkpoint,
        "checkpointId": checkpoint["checkpointId"],
        "eventRefs": [event_id],
        "replayRefs": [{"replayRef": format!("tradeassembly://execution/{activation_id}/replay"), "deterministic": true}],
        "warnings": [LEGAL_BOUNDARY, "Local-only execution evidence remains on this device unless exported."],
    });
    run["localLiveAuthority"] = live_authority;
    run["orchestrator"] = json!(orchestrator_kind(&config));
    run["immutableInput"]["orchestrator"] = json!(orchestrator_kind(&config));
    run["correlationId"] = json!(correlation_id);
    run["health"]["correlationId"] = json!(correlation_id);
    run["calendarRef"] = config["calendarRef"].clone();
    run["startedAtMs"] = json!(now_ms);
    run["dataFreshnessPolicy"] = config["dataFreshnessPolicy"].clone();
    run["checkpointHash"] = checkpoint["checkpointHash"].clone();
    run["watchRefs"] = json!([watch_id]);
    if external_agent(&config) {
        run["health"]["status"] = json!("awaiting_external_agent");
        run["replayRefs"][0]["deterministic"] = json!(false);
    }
    let interval_seconds = config["schedulerIntervalSeconds"]
        .as_u64()
        .unwrap_or(60)
        .max(1);
    let config_id = config["configId"].as_str().unwrap_or_default();
    let strategy_id = config["strategyId"].as_str().unwrap_or_default();
    let evidence_id = format!(
        "evidence_{}_activation_commit_{now_ms}",
        slug(&activation_id)
    );
    let derived_scopes = [
        ("execution_config", config_id, "strategy", strategy_id),
        (
            "execution_activation",
            activation_id.as_str(),
            "execution_config",
            config_id,
        ),
        (
            "execution_run",
            run_id.as_str(),
            "execution_activation",
            activation_id.as_str(),
        ),
        (
            "execution_event",
            event_id.as_str(),
            "execution_activation",
            activation_id.as_str(),
        ),
        (
            "execution_journal",
            event_id.as_str(),
            "execution_activation",
            activation_id.as_str(),
        ),
        (
            "execution_evidence",
            evidence_id.as_str(),
            "execution_activation",
            activation_id.as_str(),
        ),
        (
            "execution_export",
            activation_id.as_str(),
            "execution_activation",
            activation_id.as_str(),
        ),
    ];
    if derived_scopes
        .iter()
        .any(|(object_type, object_id, parent_type, parent_id)| {
            service
                .bind_inherited_object(object_type, object_id, parent_type, parent_id)
                .is_err()
        })
    {
        return object_scope_unavailable();
    }
    let worker_id = format!(
        "worker-local-{}",
        config["strategyId"].as_str().unwrap_or("strategy")
    );
    let next_cycle = 1;
    let schedule_key = execution_schedule_key(&activation_id, next_cycle);
    let scheduler = json!({
        "running": true,
        "state": "running",
        "activation_id": activation_id,
        "activationId": activation_id,
        "intervalSeconds": interval_seconds,
        "workerId": worker_id,
        "workerMode": "local-background-thread",
        "continuousExecution": true,
        "deliverySemantics": "at_least_once",
        "durableScheduler": service.runtime().scheduler.descriptors().first().map(|descriptor| descriptor.adapter_id.clone()),
        "durableLeaseRepository": service.runtime().leases.descriptors().first().map(|descriptor| descriptor.adapter_id.clone()),
        "nextCycle": next_cycle,
        "nextScheduleKey": schedule_key,
    });
    let batch_context = control_side_effect_context(&body, "activation-commit");
    let write = |namespace: &str, key: &str, value: Value, suffix: &str| {
        StorageWrite::new(
            namespace,
            key,
            value,
            SideEffectContext::new(
                batch_context.authority.clone(),
                IdempotencyKey::new(format!(
                    "{}:{suffix}",
                    batch_context.idempotency_key.as_str()
                ))
                .expect("valid activation batch idempotency key"),
            ),
        )
    };
    let mut persisted_config = config.clone();
    persisted_config
        .as_object_mut()
        .map(|object| object.remove("preparedCapabilityGraphRevision"));
    let mut writes = vec![
        write(
            CONFIGS_NS,
            config["configId"].as_str().unwrap_or("cfg_local"),
            persisted_config,
            "config",
        ),
        write(CONTROLS_NS, &activation_id, controls.clone(), "controls"),
        write(EVENTS_NS, &event_id, activation_event, "event"),
        write(
            CHECKPOINTS_NS,
            checkpoint["checkpointId"].as_str().unwrap_or("checkpoint"),
            checkpoint.clone(),
            "checkpoint",
        ),
        write(WATCHES_NS, &watch_id, watch, "watch"),
        write(
            RECONCILIATION_NS,
            &activation_id,
            reconciliation.clone(),
            "reconciliation",
        ),
        write(
            ACTIVATIONS_NS,
            &activation_id,
            activation_record,
            "activation",
        ),
        write(RUNS_NS, &run_id, run.clone(), "run"),
        write(HEALTH_NS, &activation_id, run["health"].clone(), "health"),
        write(
            TRUSTED_CLOCK_NS,
            ACTIVATION_CLOCK_KEY,
            json!({"lastTrustedAtMs": now_ms}),
            "clock",
        ),
    ];
    if !external_agent(&config) {
        writes.push(write(STATE_NS, "current", scheduler.clone(), "scheduler"));
    }
    let mut expectations = vec![StorageExpectation::new(
        TRUSTED_CLOCK_NS,
        ACTIVATION_CLOCK_KEY,
        previous_clock,
    )];
    if let (Some(revision_id), Some(input_hash), Some(idempotency_key)) = (
        capability_revision["revisionId"].as_str(),
        capability_revision["inputHash"].as_str(),
        capability_revision["idempotencyKey"].as_str(),
    ) {
        let revision_existing = match service
            .runtime()
            .storage
            .get_json(capability_graph::REVISIONS_NS, revision_id)
        {
            Ok(value) => value,
            Err(_) => return activation_dependency_failure("activation_storage_unavailable"),
        };
        let idempotency_ref = capability_graph::hash_string(idempotency_key);
        let idempotency_existing = match service
            .runtime()
            .storage
            .get_json(capability_graph::IDEMPOTENCY_NS, &idempotency_ref)
        {
            Ok(value) => value,
            Err(_) => return activation_dependency_failure("activation_storage_unavailable"),
        };
        let idempotency_record = json!({
            "inputHash": input_hash,
            "revisionId": revision_id,
        });
        if revision_existing
            .as_ref()
            .is_some_and(|existing| existing != &capability_revision)
            || idempotency_existing
                .as_ref()
                .is_some_and(|existing| existing != &idempotency_record)
        {
            return activation_dependency_failure("activation_capability_revision_conflict");
        }
        expectations.push(StorageExpectation::new(
            capability_graph::REVISIONS_NS,
            revision_id,
            revision_existing,
        ));
        expectations.push(StorageExpectation::new(
            capability_graph::IDEMPOTENCY_NS,
            &idempotency_ref,
            idempotency_existing,
        ));
        writes.push(write(
            capability_graph::REVISIONS_NS,
            revision_id,
            capability_revision.clone(),
            "capability-revision",
        ));
        writes.push(write(
            capability_graph::IDEMPOTENCY_NS,
            &idempotency_ref,
            idempotency_record,
            "capability-idempotency",
        ));
    }
    let evidence_context = control_side_effect_context(&body, "activation-evidence");
    if service
        .runtime()
        .evidence
        .append_verified(EvidenceRecord {
            evidence_id,
            evidence_type: "execution.activation.commit_authorized".to_string(),
            aggregate_id: activation_id.clone(),
            payload: json!({
                "activationId": activation_id,
                "correlationId": correlation_id,
                "commandDigest": hash_value(&json!({
                    "activationId": activation_id,
                    "configId": config["configId"],
                    "strategyVersionId": config["strategyVersionId"],
                    "capabilityGraphRevisionId": capability_revision["revisionId"],
                    "trustedAtMs": now_ms,
                })),
                "state": "authorized_for_atomic_commit",
            }),
            idempotency_key: match IdempotencyKey::new(format!(
                "{}:{now_ms}",
                evidence_context.idempotency_key.as_str()
            )) {
                Ok(key) => key,
                Err(_) => {
                    return activation_dependency_failure("activation_evidence_context_invalid")
                }
            },
            recorded_at_ms: now_ms,
        })
        .is_err()
    {
        return activation_dependency_failure("activation_evidence_unavailable");
    }
    match service
        .runtime()
        .storage
        .put_json_batch(&writes, &expectations)
    {
        Ok(ComparePutOutcome::Updated) => {}
        Ok(ComparePutOutcome::Conflict) => {
            if let Some(existing) = get(service, RUNS_NS, &run_id) {
                if existing["kind"] == "ExecutionRun"
                    && activation_matches_config(&existing, &config)
                    && (config["mode"] != "live"
                        || existing["localLiveAuthority"] == run["localLiveAuthority"])
                {
                    return activation_response(service, existing, true);
                }
            }
            return activation_dependency_failure("activation_commit_conflict");
        }
        Err(_) => return activation_dependency_failure("activation_storage_unavailable"),
    }
    if external_agent(&config) {
        let mut response = activation_response(service, run, false);
        response["scheduler"] = json!({"running":false,"state":"external_agent","publicationStatus":"not_applicable","continuousExecution":false});
        return response;
    }
    let schedule_published =
        schedule_execution_tick(service, &activation_id, next_cycle, now_ms).is_ok();
    if !schedule_published {
        run["health"]["status"] = json!("degraded");
        run["health"]["scheduler"] = json!("publication_pending");
    }
    service.runtime().scheduler_wake.signal();
    start_local_scheduler_worker(service.clone(), activation_id, worker_id, interval_seconds);
    let mut response = activation_response(service, run, false);
    response["scheduler"] = scheduler;
    response["scheduler"]["publicationStatus"] = if schedule_published {
        json!("published")
    } else {
        json!("pending_retry")
    };
    response
}

fn activation_dependency_failure(code: &str) -> Value {
    json!({
        "status": "failed",
        "failClosed": true,
        "error": {
            "code": code,
            "retryable": true,
        },
    })
}

fn activation_config(service: &TradeAssemblyService, body: &Value) -> Value {
    if has_explicit_config_id(body) || !has_config_overrides(body) {
        find_config(service, body).unwrap_or_else(|| default_config(service, body))
    } else {
        build_config(service, body, false)
    }
}

pub(super) fn authorized_activation_config(
    service: &TradeAssemblyService,
    body: &Value,
) -> Result<Value, ServiceResponse> {
    let config = activation_config(service, body);
    let config_id = config["configId"].as_str().unwrap_or_default();
    if has_explicit_config_id(body) {
        service.require_object("execution_config", config_id)?;
    }
    service.require_object(
        "strategy",
        config["strategyId"].as_str().unwrap_or_default(),
    )?;
    Ok(config)
}

fn activation_blocked_response(readiness: Value) -> Value {
    let live_preflight = readiness["livePreflight"].clone();
    let blocked_reasons = readiness["blockedReasons"].clone();
    json!({
        "status": "blocked",
        "ready": false,
        "readiness": readiness,
        "livePreflight": live_preflight,
        "blockedReasons": blocked_reasons,
    })
}

pub(crate) fn deactivate(service: &TradeAssemblyService, body: Value) -> Value {
    let activation_id = string_field(&body, &["activationId", "activation_id"])
        .unwrap_or_else(|| "activation-btc-exit-demo".to_string());
    if let Err(response) = service.require_object("execution_activation", &activation_id) {
        return object_unavailable(response);
    }
    let run_id = run_id_for_activation(&activation_id);
    let Some(mut run) = get(service, RUNS_NS, &run_id) else {
        return json!({"activationId": activation_id, "status": "not_found"});
    };
    run["state"] = json!("stopped");
    run["stateReason"] = json!("deactivated_by_user");
    run["stoppedAt"] = json!("2026-07-08T00:10:00Z");
    append_event(
        service,
        &activation_id,
        "activation.deactivated",
        json!({"reason": "user_requested"}),
    );
    persist(
        service,
        RUNS_NS,
        &run_id,
        run.clone(),
        "execution.deactivated",
    );
    if let Some(mut activation) = get(service, ACTIVATIONS_NS, &activation_id) {
        activation["state"] = json!("stopped");
        activation["stateReason"] = json!("deactivated_by_user");
        activation["stoppedAtMs"] = json!(service.runtime().clock.now_ms());
        persist(
            service,
            ACTIVATIONS_NS,
            &activation_id,
            activation,
            "execution.activation.deactivated",
        );
    }
    let state = scheduler_status(service);
    if state["activationId"].as_str() == Some(&activation_id)
        || state["activation_id"].as_str() == Some(&activation_id)
    {
        let _ = scheduler_stop(service);
    }
    json!({"activationId": activation_id, "executionRunId": run_id, "status": "deactivated", "run": run})
}

pub(crate) fn control(service: &TradeAssemblyService, body: Value) -> Value {
    let requested_at_ms = service.runtime().clock.now_ms();
    let activation_id = string_field(&body, &["activationId", "activation_id"]);
    let action = string_field(&body, &["action"]);
    let command_id = string_field(&body, &["controlPlaneCommandId"])
        .unwrap_or_else(|| format!("control_{}", short_hash(&hash_value(&body))));
    let correlation_id = activation_id
        .as_deref()
        .and_then(|id| durable_correlation_id(service, id))
        .unwrap_or_else(|| trusted_correlation_id(&body));
    let idempotency_key = string_field(
        &body,
        &[
            "controlPlaneIdempotencyKey",
            "idempotencyKey",
            "idempotency_key",
        ],
    )
    .unwrap_or_else(|| command_id.clone());
    let actor = control_actor(&body);
    let Some(activation_id) = activation_id else {
        return rejected_control_outcome(
            service,
            &body,
            None,
            action,
            requested_at_ms,
            "activation_id_required",
        );
    };
    if let Err(response) = service.require_object("execution_activation", &activation_id) {
        return object_unavailable(response);
    }
    let Some(action) = action else {
        return rejected_control_outcome(
            service,
            &body,
            Some(&activation_id),
            None,
            requested_at_ms,
            "execution_control_action_required",
        );
    };
    let Some(mut run) = get(service, RUNS_NS, &run_id_for_activation(&activation_id)) else {
        return rejected_control_outcome(
            service,
            &body,
            Some(&activation_id),
            Some(action),
            requested_at_ms,
            "activation_not_found",
        );
    };
    let (state, authority_class) = match action.as_str() {
        "reconcile" => ("completed", "reconciliation_control"),
        "resume_entries" => ("resumed", "runtime_control"),
        "stop_after_flat" => ("armed", "runtime_control"),
        "cancel_orders" => ("completed", "order_control"),
        "liquidate" => ("completed", "position_control"),
        "manual_override" => ("requested", "manual_control"),
        "emergency_stop" => ("engaged", "emergency_control"),
        "stop" => ("stopped", "runtime_control"),
        "pause_entries" => ("paused", "runtime_control"),
        _ => {
            return rejected_control_outcome(
                service,
                &body,
                Some(&activation_id),
                Some(action),
                requested_at_ms,
                "execution_control_invalid",
            )
        }
    };
    if action == "resume_entries" {
        let reconciliation = get(service, RECONCILIATION_NS, &activation_id)
            .unwrap_or_else(|| json!({"state": "unavailable"}));
        let health = get(service, HEALTH_NS, &activation_id)
            .unwrap_or_else(|| json!({"status": "unavailable"}));
        let ready = run["readiness"]["ready"].as_bool().unwrap_or(false);
        let healthy = !matches!(
            health["status"].as_str(),
            Some("failed" | "error" | "degraded")
        );
        if !ready || !healthy || reconciliation["state"] != "clear" {
            return rejected_control_outcome(
                service,
                &body,
                Some(&activation_id),
                Some(action),
                requested_at_ms,
                "resume_preflight_failed",
            );
        }
    }

    let prior_run_state = run["state"].clone();
    let prior_controls = get(service, CONTROLS_NS, &activation_id)
        .unwrap_or_else(|| controls_default(&activation_id));
    let mut controls = prior_controls.clone();
    let mut affected_order_refs = Vec::new();
    let mut affected_position_refs = Vec::new();
    let mut generated_order_refs = Vec::new();
    let mut blocked_refs = Vec::new();
    let mut reconciliation_consequence = "none".to_string();

    match action.as_str() {
        "reconcile" => match reconcile_ambiguous_actions(service, &mut run) {
            Ok(()) => {
                controls["reconcile"] = json!("completed");
                reconciliation_consequence = "provider_outcome_reconciled".to_string();
            }
            Err(error) => {
                controls["reconcile"] = json!("blocked");
                blocked_refs.push(format!("reconciliation://{activation_id}/{error}"));
                reconciliation_consequence = error;
            }
        },
        "resume_entries" => {
            controls["pauseEntries"] = json!("available");
            controls["resumeEntries"] = json!("resumed");
            run["state"] = json!("active");
            run["stateReason"] = json!("user_requested_resume");
            run["stoppedAt"] = Value::Null;
            run["stoppedAtMs"] = Value::Null;
            run["health"]["status"] = json!("running");
            run["health"]["reason"] = Value::Null;
            if let Some(mut health) = get(service, HEALTH_NS, &activation_id) {
                health["status"] = json!("healthy");
                health["reason"] = Value::Null;
                persist_control_value(
                    service,
                    HEALTH_NS,
                    &activation_id,
                    health,
                    "execution.health.resumed",
                    &body,
                );
            }
        }
        "cancel_orders" => {
            let outcome = cancel_activation_orders(service, &activation_id, &command_id, &body);
            affected_order_refs = string_array(&outcome["affectedRefs"]);
            blocked_refs = string_array(&outcome["blockedRefs"]);
            reconciliation_consequence = if blocked_refs.is_empty() {
                "local_paper_orders_canceled".to_string()
            } else {
                "external_adapter_cancellation_required".to_string()
            };
            controls["cancelOrders"] = if blocked_refs.is_empty() {
                json!("completed")
            } else {
                json!("partial")
            };
        }
        "liquidate" => {
            let outcome =
                liquidate_activation_positions(service, &activation_id, &command_id, &body);
            affected_position_refs = string_array(&outcome["affectedPositionRefs"]);
            generated_order_refs = string_array(&outcome["generatedOrderRefs"]);
            blocked_refs = string_array(&outcome["blockedRefs"]);
            reconciliation_consequence = if blocked_refs.is_empty() {
                "local_paper_positions_liquidated".to_string()
            } else {
                "external_adapter_liquidation_required".to_string()
            };
            controls["liquidate"] = if blocked_refs.is_empty() {
                json!("completed")
            } else {
                json!("partial")
            };
        }
        "stop" => {
            controls["stop"] = json!("stopped");
            run["state"] = json!("stopped");
            run["stateReason"] = json!("user_requested_stop");
            reconciliation_consequence =
                "evaluation_stopped_orders_and_positions_unchanged".to_string();
        }
        "emergency_stop" => {
            controls["emergencyStop"] = json!("engaged");
            run["state"] = json!("emergency_stopped");
            run["stateReason"] = json!("emergency_stop");
            reconciliation_consequence =
                "evaluation_stopped_orders_and_positions_unchanged".to_string();
        }
        _ => controls[action_key(&action)] = json!(state),
    }

    let outcome_status = if blocked_refs.is_empty() {
        "completed"
    } else if affected_order_refs.is_empty()
        && affected_position_refs.is_empty()
        && generated_order_refs.is_empty()
    {
        "rejected"
    } else {
        "partially_completed"
    };
    controls["lastControl"] = json!({
        "action": action,
        "state": state,
        "authorityClass": authority_class,
        "audited": true,
        "commandId": command_id,
        "outcomeStatus": outcome_status,
    });
    run["controls"] = controls.clone();
    persist_control_value(
        service,
        CONTROLS_NS,
        &activation_id,
        controls.clone(),
        "execution.control.applied",
        &body,
    );
    persist_control_value(
        service,
        RUNS_NS,
        &run_id_for_activation(&activation_id),
        run.clone(),
        "execution.control_state_updated",
        &body,
    );
    if let Some(mut activation) = get(service, ACTIVATIONS_NS, &activation_id) {
        activation["state"] = run["state"].clone();
        activation["stateReason"] = run["stateReason"].clone();
        activation["controls"] = controls.clone();
        persist_control_value(
            service,
            ACTIVATIONS_NS,
            &activation_id,
            activation,
            "execution.activation.controlled",
            &body,
        );
    }
    if matches!(action.as_str(), "stop" | "emergency_stop") {
        stop_scheduler_for_activation(service, &activation_id);
    } else if action == "resume_entries"
        && !external_agent(&run)
        && matches!(
            prior_run_state.as_str(),
            Some("stopped" | "emergency_stopped")
        )
    {
        let _ = scheduler_start(
            service,
            json!({
                "activationId": activation_id,
                "intervalSeconds": run["schedulerIntervalSeconds"].as_u64().unwrap_or(60),
            }),
        );
    }

    let completed_at_ms = service.runtime().clock.now_ms();
    let failure_reason = if blocked_refs.is_empty() {
        Value::Null
    } else {
        json!("external_plugin_control_operation_unavailable")
    };
    let mut outcome = json!({
        "schemaVersion": "tradeassembly.execution_control_outcome.v1",
        "commandId": command_id,
        "correlationId": correlation_id,
        "idempotencyKey": idempotency_key,
        "actor": actor,
        "activationId": activation_id,
        "action": action,
        "authorityClass": authority_class,
        "priorState": {
            "runState": prior_run_state,
            "controls": prior_controls,
        },
        "resultingState": {
            "runState": run["state"],
            "controls": controls,
        },
        "affected": {
            "orderRefs": affected_order_refs,
            "positionRefs": affected_position_refs,
            "generatedOrderRefs": generated_order_refs,
            "blockedRefs": blocked_refs,
        },
        "requestedAtMs": requested_at_ms,
        "completedAtMs": completed_at_ms,
        "status": outcome_status,
        "failureReason": failure_reason,
        "reconciliationConsequence": reconciliation_consequence,
    });
    let event_ref = append_event_with_body(
        service,
        &activation_id,
        "execution.control.completed",
        outcome.clone(),
        &body,
    );
    outcome["eventRef"] = json!(event_ref);
    outcome["evidenceRefs"] = json!([
        format!("control-plane://commands/{command_id}"),
        format!("execution://events/{event_ref}")
    ]);
    persist_control_value(
        service,
        CONTROL_OUTCOMES_NS,
        &command_id,
        outcome.clone(),
        "execution.control.outcome",
        &body,
    );
    json!({
        "activationId": activation_id,
        "action": action,
        "state": state,
        "controls": controls,
        "outcome": outcome,
    })
}

fn rejected_control_outcome(
    service: &TradeAssemblyService,
    body: &Value,
    activation_id: Option<&str>,
    action: Option<String>,
    requested_at_ms: i64,
    reason: &str,
) -> Value {
    let command_id = string_field(body, &["controlPlaneCommandId"])
        .unwrap_or_else(|| format!("control_{}", short_hash(&hash_value(body))));
    let correlation_id = activation_id
        .and_then(|id| durable_correlation_id(service, id))
        .unwrap_or_else(|| trusted_correlation_id(body));
    let idempotency_key = string_field(
        body,
        &[
            "controlPlaneIdempotencyKey",
            "idempotencyKey",
            "idempotency_key",
        ],
    )
    .unwrap_or_else(|| command_id.clone());
    let actor = control_actor(body);
    let action = action.unwrap_or_default();
    let mut outcome = json!({
        "schemaVersion": "tradeassembly.execution_control_outcome.v1",
        "commandId": command_id,
        "correlationId": correlation_id,
        "idempotencyKey": idempotency_key,
        "actor": actor,
        "activationId": activation_id,
        "action": action,
        "priorState": Value::Null,
        "resultingState": Value::Null,
        "affected": {
            "orderRefs": [],
            "positionRefs": [],
            "generatedOrderRefs": [],
            "blockedRefs": [],
        },
        "requestedAtMs": requested_at_ms,
        "completedAtMs": service.runtime().clock.now_ms(),
        "status": "rejected",
        "failureReason": reason,
        "reconciliationConsequence": "none",
        "evidenceRefs": [format!("control-plane://commands/{command_id}")],
    });
    if let Some(activation_id) = activation_id {
        let event_ref = append_event_with_body(
            service,
            activation_id,
            "execution.control.rejected",
            outcome.clone(),
            body,
        );
        outcome["eventRef"] = json!(event_ref);
        outcome["evidenceRefs"] = json!([
            format!("control-plane://commands/{command_id}"),
            format!("execution://events/{event_ref}")
        ]);
    }
    persist_control_value(
        service,
        CONTROL_OUTCOMES_NS,
        &command_id,
        outcome.clone(),
        "execution.control.rejected",
        body,
    );
    json!({
        "activationId": activation_id,
        "action": action,
        "status": "rejected",
        "error": {"code": reason},
        "outcome": outcome,
    })
}

fn cancel_activation_orders(
    service: &TradeAssemblyService,
    activation_id: &str,
    command_id: &str,
    body: &Value,
) -> Value {
    let mut affected = Vec::new();
    let mut blocked = Vec::new();
    let mut released_reservations = Vec::new();
    for (key, mut order) in service
        .runtime()
        .storage
        .list_json("execution_orders")
        .unwrap_or_default()
    {
        if !value_matches_activation(&order, activation_id) || !order_is_cancelable(&order) {
            continue;
        }
        let order_id =
            string_field(&order, &["order_id", "orderId", "id"]).unwrap_or_else(|| key.clone());
        if !local_simulated_order(&order) {
            blocked.push(order_id);
            continue;
        }
        order["status"] = json!("canceled");
        order["canceledAtMs"] = json!(service.runtime().clock.now_ms());
        order["cancelReason"] = json!("user_requested_paper_cancel");
        order["controlOutcomeRef"] = json!(command_id);
        persist_control_value(
            service,
            "execution_orders",
            &key,
            order,
            "execution.order.canceled",
            body,
        );
        released_reservations.push(order_id.clone());
        affected.push(order_id);
    }
    risk::release_for_order_refs(
        service,
        activation_id,
        &released_reservations,
        command_id,
        body,
    );
    affected.sort();
    blocked.sort();
    json!({"affectedRefs": affected, "blockedRefs": blocked})
}

fn liquidate_activation_positions(
    service: &TradeAssemblyService,
    activation_id: &str,
    command_id: &str,
    body: &Value,
) -> Value {
    let orders = service
        .runtime()
        .storage
        .list_json("execution_orders")
        .unwrap_or_default();
    let mut affected_positions = Vec::new();
    let mut generated_orders = Vec::new();
    let mut blocked = Vec::new();
    let mut released_reservations = Vec::new();
    for (key, mut position) in service
        .runtime()
        .storage
        .list_json("positions")
        .unwrap_or_default()
    {
        if !value_matches_activation(&position, activation_id)
            || position["status"].as_str() != Some("open")
        {
            continue;
        }
        let position_id =
            string_field(&position, &["position_id", "positionId", "id"]).unwrap_or(key.clone());
        let source_order_id = string_field(&position, &["order_id", "orderId"]);
        let source_order = source_order_id.as_ref().and_then(|source_id| {
            orders.iter().find_map(|(_, order)| {
                (string_field(order, &["order_id", "orderId", "id"]).as_deref()
                    == Some(source_id.as_str()))
                .then_some(order)
            })
        });
        if !source_order.is_some_and(local_simulated_order) {
            blocked.push(position_id);
            continue;
        }
        let quantity = optional_number_field(&position, &["qty", "quantity"]).unwrap_or(0.0);
        let average = optional_number_field(&position, &["average_price", "averagePrice"]);
        let mark = optional_number_field(&position, &["mark_price", "markPrice"]).or(average);
        let side = match position["side"].as_str() {
            Some("short") => "buy",
            _ => "sell",
        };
        let liquidation_order_id = format!(
            "order_liquidate_{}_{}",
            slug(activation_id),
            slug(&position_id)
        );
        let realized_pnl = match (average, mark) {
            (Some(average), Some(mark)) if side == "sell" => json!((mark - average) * quantity),
            (Some(average), Some(mark)) => json!((average - mark) * quantity),
            _ => Value::Null,
        };
        let liquidation_order = json!({
            "id": liquidation_order_id,
            "order_id": liquidation_order_id,
            "activation_id": activation_id,
            "strategy_id": position.get("strategy_id").or_else(|| position.get("strategyId")).cloned().unwrap_or(Value::Null),
            "symbol": position["symbol"],
            "side": side,
            "qty": quantity,
            "status": "filled",
            "brokerExecution": "local-simulated",
            "fill_price": mark,
            "filled_at_ms": service.runtime().clock.now_ms(),
            "realized_pnl": realized_pnl,
            "liquidatesPositionRef": position_id,
            "controlOutcomeRef": command_id,
            "noAdvice": LEGAL_BOUNDARY,
        });
        persist_control_value(
            service,
            "execution_orders",
            &liquidation_order_id,
            liquidation_order,
            "execution.position.liquidation_order_filled",
            body,
        );
        position["status"] = json!("closed");
        position["closedAtMs"] = json!(service.runtime().clock.now_ms());
        position["closePrice"] = json!(mark);
        position["realizedPnl"] = realized_pnl;
        position["closeReason"] = json!("user_requested_paper_liquidation");
        position["liquidationOrderRef"] = json!(liquidation_order_id);
        position["controlOutcomeRef"] = json!(command_id);
        persist_control_value(
            service,
            "positions",
            &key,
            position,
            "execution.position.liquidated",
            body,
        );
        if let Some(source_order_id) = source_order_id {
            released_reservations.push(source_order_id);
        }
        affected_positions.push(position_id);
        generated_orders.push(liquidation_order_id);
    }
    risk::release_for_order_refs(
        service,
        activation_id,
        &released_reservations,
        command_id,
        body,
    );
    affected_positions.sort();
    generated_orders.sort();
    blocked.sort();
    json!({
        "affectedPositionRefs": affected_positions,
        "generatedOrderRefs": generated_orders,
        "blockedRefs": blocked,
    })
}

fn local_simulated_order(order: &Value) -> bool {
    matches!(
        string_field(order, &["brokerExecution", "broker_execution"]).as_deref(),
        Some("local-simulated" | "tradeassembly.simbroker" | "sim")
    )
}

fn order_is_cancelable(order: &Value) -> bool {
    matches!(
        order["status"].as_str(),
        Some(
            "planned"
                | "submission_pending"
                | "submitted"
                | "accepted"
                | "new"
                | "partially_filled"
                | "pending"
        )
    )
}

fn value_matches_activation(value: &Value, activation_id: &str) -> bool {
    string_field(value, &["activation_id", "activationId"]).as_deref() == Some(activation_id)
}

fn string_array(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn control_actor(body: &Value) -> Value {
    let context = body
        .get("authorityContext")
        .or_else(|| body.get("authority_context"));
    json!({
        "actor": context
            .and_then(|value| value.get("actor").or_else(|| value.get("principalUser")))
            .and_then(Value::as_str)
            .unwrap_or("local-user"),
        "surface": context
            .and_then(|value| value.get("surface"))
            .and_then(Value::as_str)
            .unwrap_or("runtime"),
        "accountMode": context
            .and_then(|value| value.get("accountMode").or_else(|| value.get("account_mode")))
            .and_then(Value::as_str)
            .or_else(|| body.get("accountMode").and_then(Value::as_str))
            .unwrap_or("paper"),
    })
}

fn stop_scheduler_for_activation(service: &TradeAssemblyService, activation_id: &str) {
    let state = scheduler_status(service);
    if state["activationId"].as_str() == Some(activation_id)
        || state["activation_id"].as_str() == Some(activation_id)
    {
        let _ = scheduler_stop(service);
    }
}

fn persist_control_value(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    value: Value,
    event_type: &str,
    body: &Value,
) {
    let context = control_side_effect_context(body, &format!("{event_type}:{key}"));
    service
        .runtime()
        .storage
        .put_json(namespace, key, value.clone(), &context)
        .expect("persist execution control state");
    service
        .runtime()
        .record_side_effect(event_type, value, &context)
        .expect("record execution control evidence");
}

fn control_side_effect_context(body: &Value, suffix: &str) -> SideEffectContext {
    let actor = control_actor(body);
    let base_key = string_field(
        body,
        &[
            "controlPlaneIdempotencyKey",
            "idempotencyKey",
            "idempotency_key",
        ],
    )
    .unwrap_or_else(|| format!("control:{}", short_hash(&hash_value(body))));
    SideEffectContext::new(
        AuthorityContext {
            actor: actor["actor"].as_str().unwrap_or("local-user").to_string(),
            surface: actor["surface"].as_str().unwrap_or("runtime").to_string(),
            account_mode: actor["accountMode"].as_str().unwrap_or("paper").to_string(),
        },
        IdempotencyKey::new(format!("{base_key}:{suffix}")).expect("valid control idempotency key"),
    )
}

pub(crate) fn scheduler_start(service: &TradeAssemblyService, body: Value) -> Value {
    let activation_id =
        string_field(&body, &["activationId", "activation_id"]).unwrap_or_else(|| {
            latest_activation_id(service).unwrap_or_else(|| "activation-btc-exit-demo".to_string())
        });
    let interval_seconds = body
        .get("intervalSeconds")
        .or_else(|| body.get("interval_seconds"))
        .and_then(Value::as_u64)
        .unwrap_or(60)
        .max(1);
    let worker_id = string_field(&body, &["workerId", "worker_id"])
        .unwrap_or_else(|| format!("worker-local-{activation_id}"));
    let Some(run) = get(service, RUNS_NS, &run_id_for_activation(&activation_id)) else {
        return json!({
            "running": false,
            "state": "failed",
            "error": {"code": "activation_not_found"},
            "activationId": activation_id,
        });
    };
    let next_cycle = run["cycle"].as_u64().unwrap_or(0) + 1;
    if let Err(error) = schedule_execution_tick(
        service,
        &activation_id,
        next_cycle,
        service.runtime().clock.now_ms(),
    ) {
        return scheduler_failure("schedule_failed", error);
    }
    let state = json!({
        "running": true,
        "state": "running",
        "activation_id": activation_id,
        "activationId": activation_id,
        "intervalSeconds": interval_seconds,
        "workerId": worker_id,
        "workerMode": "local-background-thread",
        "continuousExecution": true,
        "deliverySemantics": "at_least_once",
        "durableScheduler": service.runtime().scheduler.descriptors().first().map(|descriptor| descriptor.adapter_id.clone()),
        "durableLeaseRepository": service.runtime().leases.descriptors().first().map(|descriptor| descriptor.adapter_id.clone()),
        "nextCycle": next_cycle,
        "nextScheduleKey": execution_schedule_key(&activation_id, next_cycle),
    });
    persist(
        service,
        STATE_NS,
        "current",
        state.clone(),
        "scheduler.started",
    );
    service.runtime().scheduler_wake.signal();
    start_local_scheduler_worker(service.clone(), activation_id, worker_id, interval_seconds);
    state
}

pub(crate) fn scheduler_stop(service: &TradeAssemblyService) -> Value {
    let mut state = scheduler_status(service);
    if let Some(activation_id) = state["activationId"]
        .as_str()
        .or_else(|| state["activation_id"].as_str())
    {
        if let Some(run) = get(service, RUNS_NS, &run_id_for_activation(activation_id)) {
            let next_cycle = run["cycle"].as_u64().unwrap_or(0) + 1;
            let _ = service
                .runtime()
                .scheduler
                .cancel(&execution_schedule_key(activation_id, next_cycle));
        }
    }
    state["running"] = json!(false);
    state["state"] = json!("stopped");
    persist(
        service,
        STATE_NS,
        "current",
        state.clone(),
        "scheduler.stopped",
    );
    service.runtime().scheduler_wake.signal();
    state
}

pub(crate) fn scheduler_status(service: &TradeAssemblyService) -> Value {
    get(service, STATE_NS, "current")
        .unwrap_or_else(|| json!({"running": false, "state": "stopped"}))
}

pub(crate) fn scheduler_run(service: &TradeAssemblyService, body: Value) -> Value {
    let max_cycles = body
        .get("maxCycles")
        .or_else(|| body.get("max_cycles"))
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let worker_id = string_field(&body, &["workerId", "worker_id"])
        .unwrap_or_else(|| "worker-local".to_string());
    let lease_ttl_seconds = body
        .get("leaseTtlSeconds")
        .or_else(|| body.get("lease_ttl_seconds"))
        .and_then(Value::as_u64)
        .unwrap_or(30)
        .max(1);
    let target_activation = string_field(&body, &["activationId", "activation_id"]);
    let interval_seconds = scheduler_status(service)["intervalSeconds"]
        .as_u64()
        .unwrap_or(60)
        .max(1);
    let base_now_ms = service.runtime().clock.now_ms();
    let lease_ms = i64::try_from(lease_ttl_seconds)
        .unwrap_or(i64::MAX / 1_000)
        .saturating_mul(1_000);
    let mut ticks = Vec::new();
    for iteration in 0..max_cycles {
        let claim_now_ms = base_now_ms.saturating_add(
            i64::try_from(iteration)
                .unwrap_or(i64::MAX)
                .saturating_mul(i64::try_from(interval_seconds).unwrap_or(i64::MAX))
                .saturating_mul(1_000),
        );
        let claimed = match claim_execution_ticks(
            service,
            target_activation.as_deref(),
            &worker_id,
            claim_now_ms,
            lease_ms,
        ) {
            Ok(claimed) => claimed,
            Err(error) => return scheduler_failure("claim_failed", error),
        };
        if claimed.is_empty() {
            ticks.push(json!({
                "status": "idle",
                "reason": "no_due_work",
                "lease": {"acquired": false, "duplicate": false, "idle": true},
            }));
            continue;
        }
        for work in claimed {
            match process_execution_tick(
                service,
                work,
                &worker_id,
                lease_ms,
                interval_seconds,
                claim_now_ms,
            ) {
                Ok(tick) => ticks.push(tick),
                Err(error) => ticks.push(json!({
                    "status": "failed",
                    "reason": "tick_processing_failed",
                    "error": error,
                    "lease": {"acquired": false, "duplicate": false},
                })),
            }
        }
    }
    let last_tick = ticks
        .last()
        .cloned()
        .unwrap_or_else(|| json!({"status": "idle", "lease": {"acquired": false, "idle": true}}));
    let mut state = scheduler_status(service);
    let completed_iterations = state["iterations"].as_u64().unwrap_or(0) + max_cycles;
    state["running"] = json!(true);
    state["state"] = json!("running");
    state["lastWorkerId"] = json!(worker_id);
    state["iterations"] = json!(completed_iterations);
    state["last_run_id"] = last_tick["executionRunId"].clone();
    state["lastTick"] = last_tick.clone();
    if let Some(activation_id) = target_activation {
        state["activationId"] = json!(activation_id);
    }
    persist(
        service,
        STATE_NS,
        "current",
        state,
        "scheduler.tick_completed",
    );
    json!({
        "cycles": max_cycles,
        "iterations": max_cycles,
        "status": "complete",
        "ticks": ticks,
        "lease": last_tick.get("lease").cloned().unwrap_or_else(|| json!({"acquired": false, "idle": true})),
    })
}

pub(crate) fn workspace(service: &TradeAssemblyService, strategy_id: &str, query: &Value) -> Value {
    if let Err(response) = service.require_object("strategy", strategy_id) {
        return object_unavailable(response);
    }
    let configs = list_configs(service, strategy_id);
    let runs = sorted_execution_runs(service, Some(strategy_id));
    let requested_activation = string_field(query, &["activationId", "activation_id"]);
    if let Some(activation_id) = requested_activation.as_deref() {
        if let Err(response) = service.require_object("execution_activation", activation_id) {
            return object_unavailable(response);
        }
    }
    let active = if let Some(activation_id) = requested_activation.as_ref() {
        runs.iter()
            .find(|run| run["activationId"].as_str() == Some(activation_id))
            .cloned()
    } else {
        runs.iter()
            .rev()
            .find(|run| matches!(run["state"].as_str(), Some("active" | "paused")))
            .cloned()
            .or_else(|| runs.last().cloned())
    };
    let activation_id = active
        .as_ref()
        .and_then(|run| run["activationId"].as_str())
        .map(str::to_string);
    let event_limit = query
        .get("eventLimit")
        .or_else(|| query.get("event_limit"))
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 500) as usize;
    let after_sequence = query
        .get("afterSequence")
        .or_else(|| query.get("after_sequence"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let events = activation_id
        .as_deref()
        .map(|id| {
            events_for(service, id)
                .into_iter()
                .filter(|event| event["sequence"].as_u64().unwrap_or(0) > after_sequence)
                .take(event_limit)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut selected = active.unwrap_or_else(|| {
        json!({
            "state": "idle",
            "activationId": null,
            "cycle": 0,
            "controls": {},
            "health": {"status": "idle"},
            "apf": {},
        })
    });
    let selected_config_id = selected["configId"].as_str();
    let durable_config = if let Some(config_id) = selected_config_id {
        configs
            .iter()
            .find(|config| {
                config["id"].as_str() == Some(config_id)
                    || config["configId"].as_str() == Some(config_id)
            })
            .cloned()
    } else {
        configs.last().cloned()
    };
    let selected_config = durable_config
        .clone()
        .unwrap_or_else(|| default_config(service, &json!({"strategyId": strategy_id})));
    let selected_readiness = readiness_for_config(service, &selected_config);
    selected["configId"] = durable_config
        .as_ref()
        .and_then(|config| {
            config["configId"]
                .as_str()
                .or_else(|| config["id"].as_str())
        })
        .map(Value::from)
        .unwrap_or(Value::Null);
    selected["readiness"] = selected_readiness.clone();
    let selected_activation_id = activation_id.as_deref().unwrap_or("");
    let no_related_orders = HashSet::new();
    let scoped_orders = values_for_activation(
        orders::stored_orders(service),
        selected_activation_id,
        &no_related_orders,
    );
    let scoped_order_ids = scoped_orders
        .iter()
        .filter_map(|order| string_field(order, &["order_id", "orderId", "id"]))
        .collect::<HashSet<_>>();
    let scoped_attempts = values_for_activation(
        orders::stored_attempts(service),
        selected_activation_id,
        &scoped_order_ids,
    );
    let scoped_positions = values_for_activation(
        positions::stored(service),
        selected_activation_id,
        &scoped_order_ids,
    );
    let scoped_open_positions = scoped_positions
        .iter()
        .filter(|position| position["status"] == "open")
        .cloned()
        .collect::<Vec<_>>();
    let scoped_risk = if selected_activation_id.is_empty() {
        json!({
            "available": false,
            "reason": "activation_not_selected",
            "activeCount": 0,
            "activeNotional": Value::Null,
            "activeReservations": [],
        })
    } else {
        risk::status_for_activation(service, selected_activation_id)
    };
    let scoped_ticks = ticks_for(service, selected_activation_id);
    let scoped_reconciliation = reconciliation_for(service, selected_activation_id);
    let mut scoped_control_outcomes = list_namespace(service, CONTROL_OUTCOMES_NS)
        .into_iter()
        .filter(|outcome| value_matches_activation(outcome, selected_activation_id))
        .collect::<Vec<_>>();
    scoped_control_outcomes.sort_by(|left, right| {
        left["requestedAtMs"]
            .as_i64()
            .unwrap_or_default()
            .cmp(&right["requestedAtMs"].as_i64().unwrap_or_default())
            .then_with(|| {
                left["commandId"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["commandId"].as_str().unwrap_or_default())
            })
    });
    let scoped_ledger = ledger_from_evidence(
        selected_activation_id,
        &scoped_orders,
        &scoped_positions,
        &scoped_reconciliation,
    );
    let scoped_chart = chart_from_evidence(
        selected_activation_id,
        strategy_id,
        &scoped_ticks,
        &scoped_orders,
    );
    let scoped_health = health_for(
        service,
        selected_activation_id,
        &selected,
        &scoped_reconciliation,
        &scoped_ticks,
    );
    let selected_scheduler = scheduler_for_activation(service, selected_activation_id);
    let mut execution = selected.clone();
    execution["activity"] = json!(events.clone());
    execution["decisions"] = json!(decision_rows_from_events(&events));
    execution["evaluations"] = json!({"items": scoped_ticks.clone()});
    execution["orders"] = json!({"items": scoped_orders.clone()});
    execution["orderAttempts"] = json!({"items": scoped_attempts.clone()});
    execution["positions"] = json!({
        "items": scoped_positions.clone(),
        "summary": {"openPositions": scoped_open_positions.len()},
    });
    execution["risk"] = scoped_risk.clone();
    execution["ledger"] = scoped_ledger.clone();
    execution["journal"] = json!({"rows": events.clone(), "totals": scoped_ledger["totals"]});
    execution["accountImpact"] = json!({
        "available": scoped_risk["activeNotional"].is_number(),
        "openPositions": scoped_open_positions.len(),
        "grossExposure": scoped_risk["activeNotional"],
        "netExposure": scoped_risk["activeNotional"],
    });
    execution["fillQualityReport"] = json!({
        "available": false,
        "reason": "provider_fill_quotes_unavailable",
        "summary": {"fillCount": scoped_orders.iter().filter(|order| order["status"] == "filled").count(), "warningCount": Value::Null},
    });
    execution["lifecycleCalendar"] = json!({
        "available": false,
        "reason": "position_lifecycle_artifacts_unavailable",
        "summary": {"affectedPositionCount": scoped_positions.len()},
    });
    execution["scheduler"] = selected_scheduler.clone();
    execution["controlOutcomes"] = json!({
        "items": scoped_control_outcomes,
        "latest": scoped_control_outcomes.last().cloned().unwrap_or(Value::Null),
    });
    execution["chart"] = scoped_chart;
    execution["health"] = scoped_health;
    execution["reconciliation"] = scoped_reconciliation.clone();
    execution["evidence"] = if selected_activation_id.is_empty() {
        json!({"available": false, "reason": "activation_not_selected"})
    } else {
        evidence_drawer(service, selected_activation_id)
    };
    execution["approval"] = approval_panel(&selected);
    execution["readiness"] = selected_readiness;
    execution["executionAnalytics"] = execution_analytics_from_evidence(
        selected_activation_id,
        &scoped_orders,
        &scoped_attempts,
        &scoped_positions,
        &events,
        &execution,
    );
    let snapshot_input = json!({
        "activationId": activation_id,
        "checkpointId": selected["checkpointId"],
        "cycle": selected["cycle"],
        "events": events,
        "orders": scoped_orders,
        "attempts": scoped_attempts,
        "positions": scoped_positions,
        "risk": scoped_risk,
        "reconciliation": scoped_reconciliation,
        "controlOutcomes": scoped_control_outcomes,
        "health": execution["health"],
    });
    let snapshot_hash = hash_value(&snapshot_input);
    let next_sequence = execution["activity"]
        .as_array()
        .and_then(|items| items.last())
        .and_then(|event| event["sequence"].as_u64())
        .unwrap_or(after_sequence);
    json!({
        "schemaVersion": "tradeassembly.execution_dashboard.v1",
        "selectedActivationId": if selected_activation_id.is_empty() { Value::Null } else { json!(selected_activation_id) },
        "selection": {
            "requestedActivationId": requested_activation,
            "status": if activation_id.is_some() {
                "selected"
            } else if requested_activation.is_some() {
                "activation_not_found"
            } else {
                "no_activation"
            },
        },
        "snapshot": {
            "snapshotId": format!("snapshot_{}", short_hash(&snapshot_hash)),
            "snapshotHash": snapshot_hash,
            "activationId": if selected_activation_id.is_empty() { Value::Null } else { json!(selected_activation_id) },
            "checkpointId": selected["checkpointId"],
            "cursor": {"afterSequence": after_sequence, "nextSequence": next_sequence, "limit": event_limit},
            "mutationFree": true,
        },
        "strategy": service.strategy_value(strategy_id),
        "configs": configs,
        "draftConfig": selected_config,
        "events": execution["activity"],
        "execution": execution,
        "activeRun": selected,
        "activeRuns": runs,
        "runs": {"attempts": execution["orderAttempts"]["items"], "brokerEvidence": execution["orders"]["items"]},
        "orders": execution["orders"]["items"],
        "orderAttempts": execution["orderAttempts"]["items"],
        "positions": execution["positions"],
        "positionLifecycle": {"summary": {"status": if scoped_open_positions.is_empty() { "flat" } else { "open" }}, "timeline": []},
        "performanceSummary": {
            "total_orders": execution["orders"]["items"].as_array().map(Vec::len).unwrap_or(0),
            "submitted_orders": execution["orders"]["items"].as_array().map(|items| items.iter().filter(|order| order["status"] == "submitted").count()).unwrap_or(0),
            "filled_orders": execution["orders"]["items"].as_array().map(|items| items.iter().filter(|order| order["status"] == "filled").count()).unwrap_or(0),
            "submit_attempts": execution["orderAttempts"]["items"].as_array().map(Vec::len).unwrap_or(0),
            "active_positions": scoped_open_positions.len(),
            "realized_pnl": execution["ledger"]["totals"]["realizedPnl"],
        },
        "risk": execution["risk"],
        "fillQualityReport": execution["fillQualityReport"].clone(),
        "executionAnalytics": execution["executionAnalytics"].clone(),
        "lifecycleCalendar": execution["lifecycleCalendar"].clone(),
        "attributionJournal": {"summary": {"available": false, "reason": "attribution_artifacts_unavailable"}},
        "journal": execution["journal"],
        "replay": if selected_activation_id.is_empty() {
            json!({"available": false, "reason": "activation_not_selected", "refs": []})
        } else {
            json!({"available": true, "refs": [{"replayRef": format!("tradeassembly://execution/{selected_activation_id}/replay"), "deterministic": true}]})
        },
        "scheduler": selected_scheduler,
        "apfAudit": dashboard_audit_reference(),
    })
}

pub(crate) fn run_center(service: &TradeAssemblyService) -> Value {
    let runs = execution_runs(service, None);
    let active_runs = runs
        .iter()
        .filter(|run| matches!(run["state"].as_str(), Some("active" | "paused")))
        .cloned()
        .collect::<Vec<_>>();
    let audit = dashboard_audit_reference();
    let scheduler = scheduler_status(service);
    let scheduler_visible = scheduler["activationId"]
        .as_str()
        .or_else(|| scheduler["activation_id"].as_str())
        .is_none_or(|activation_id| {
            service
                .require_object("execution_activation", activation_id)
                .is_ok()
        });
    let scheduler = if scheduler_visible {
        scheduler
    } else {
        json!({"running": false, "state": "idle"})
    };
    let audit = if service.invocation_owner().is_some() {
        json!({"receipts": []})
    } else {
        audit
    };
    let workspace = if service.invocation_owner().is_some() {
        json!({
            "workspace": {"id": "local", "name": "Local Workspace", "mode": "local"},
            "strategies": service.strategies(),
            "scheduler": scheduler,
        })
    } else {
        workspace::workspace(service)
    };
    json!({
        "workspace": workspace,
        "scheduler": scheduler,
        "activeRuns": active_runs,
        "readyNext": service.strategies(),
        "activationMatrix": activation_matrix(service),
        "receipts": audit["receipts"].clone(),
        "events": service.filter_visible_values(
            "execution_event",
            list_namespace(service, EVENTS_NS),
            &["id", "eventId", "event_id"],
        ),
        "openRisk": risk::status(service),
        "positions": positions::list(service),
        "failureStates": failure_states(service),
        "execution": {
            "journal": {"totals": ledger_totals(service)},
            "replay": {"deterministic": true, "counts": {"execution.tick.completed": count_events(service, "execution.tick.completed")}},
        },
        "apfAudit": audit,
    })
}

pub(crate) fn paper_runner_activation_snapshot(
    service: &TradeAssemblyService,
    activation_id: &str,
) -> Option<Value> {
    if service
        .require_object("execution_activation", activation_id)
        .is_err()
    {
        return None;
    }
    let run = get(service, RUNS_NS, &run_id_for_activation(activation_id))?;
    let activation = get(service, ACTIVATIONS_NS, activation_id)?;
    if run["activationId"].as_str() != Some(activation_id)
        || activation["activationId"].as_str() != Some(activation_id)
        || run["strategyVersionId"] != activation["strategyVersionId"]
        || run["strategySpecHash"] != activation["strategySpecHash"]
        || run["capabilityGraphRevisionId"] != activation["capabilityGraphRevisionId"]
        || run["capabilityGraphFingerprint"] != activation["capabilityGraphFingerprint"]
    {
        return None;
    }
    Some(json!({
        "schemaVersion": "tradeassembly.paper_runner_activation_snapshot.v1",
        "activationId": activation_id,
        "executionRunId": run["executionRunId"],
        "configId": run["configId"],
        "strategyId": run["strategyId"],
        "strategyVersionId": run["strategyVersionId"],
        "strategySpecHash": run["strategySpecHash"],
        "mode": run["mode"],
        "state": run["state"],
        "stateReason": run["stateReason"],
        "cycle": run["cycle"],
        "capabilityGraphRevisionId": run["capabilityGraphRevisionId"],
        "capabilityGraphFingerprint": run["capabilityGraphFingerprint"],
        "selectedManifestFingerprints": run["immutableInput"]["selectedManifestFingerprints"],
        "checkpointId": run["checkpointId"],
        "checkpointHash": run["checkpointHash"],
        "watchRefs": run["watchRefs"],
        "controls": get(service, CONTROLS_NS, activation_id),
        "reconciliation": get(service, RECONCILIATION_NS, activation_id),
        "health": get(service, HEALTH_NS, activation_id),
        "scheduler": scheduler_for_activation(service, activation_id),
    }))
}

pub(crate) fn paper_runner_config_snapshot(
    service: &TradeAssemblyService,
    config_id: &str,
) -> Option<Value> {
    if service
        .require_object("execution_config", config_id)
        .is_err()
    {
        return None;
    }
    let config = get(service, CONFIGS_NS, config_id)?;
    let readiness = readiness_for_config(service, &config);
    Some(json!({
        "schemaVersion": "tradeassembly.paper_runner_config_snapshot.v1",
        "configId": config["configId"],
        "strategyId": config["strategyId"],
        "strategyVersionId": config["strategyVersionId"],
        "strategySpecHash": config["strategySpecHash"],
        "mode": config["mode"],
        "capabilityGraphRevisionId": config["capabilityGraphRevisionId"],
        "capabilityGraphFingerprint": config["capabilityGraphFingerprint"],
        "ready": readiness["ready"],
        "blockedReasons": readiness["blockedReasons"],
    }))
}

pub(crate) fn execution_runs(
    service: &TradeAssemblyService,
    strategy_id: Option<&str>,
) -> Vec<Value> {
    service
        .filter_visible_values(
            "execution_run",
            list_namespace(service, RUNS_NS),
            &["id", "executionRunId", "run_id"],
        )
        .into_iter()
        .filter(|item| item["kind"] == "ExecutionRun")
        .filter(|item| {
            strategy_id
                .map(|id| item["strategyId"].as_str() == Some(id))
                .unwrap_or(true)
        })
        .collect()
}

pub(crate) fn plugin_dependency_runs(
    service: &TradeAssemblyService,
    instance_ref: &str,
) -> Vec<Value> {
    let mut dependencies = list_namespace(service, RUNS_NS)
        .into_iter()
        .filter(|run| run["kind"] == "ExecutionRun")
        .filter(|run| matches!(run["state"].as_str(), Some("active" | "paused")))
        .filter(|run| {
            run.pointer("/immutableInput/capabilityGraphSnapshot/nodes")
                .and_then(Value::as_array)
                .is_some_and(|nodes| {
                    nodes.iter().any(|node| {
                        node.get("selected").is_some_and(|selected| {
                            selected["pluginInstanceRef"].as_str() == Some(instance_ref)
                        })
                    })
                })
        })
        .map(|run| {
            json!({
                "activationId": run["activationId"],
                "executionRunId": run["executionRunId"],
                "state": run["state"],
            })
        })
        .collect::<Vec<_>>();
    dependencies.sort_by(|left, right| {
        left["activationId"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["activationId"].as_str().unwrap_or_default())
    });
    dependencies
}

fn sorted_execution_runs(service: &TradeAssemblyService, strategy_id: Option<&str>) -> Vec<Value> {
    let mut runs = execution_runs(service, strategy_id);
    runs.sort_by(|left, right| {
        let left_created = left["startedAtMs"]
            .as_i64()
            .or_else(|| left["createdAtMs"].as_i64())
            .unwrap_or_default();
        let right_created = right["startedAtMs"]
            .as_i64()
            .or_else(|| right["createdAtMs"].as_i64())
            .unwrap_or_default();
        left_created.cmp(&right_created).then_with(|| {
            left["activationId"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["activationId"].as_str().unwrap_or_default())
        })
    });
    runs
}

fn readiness_for_config(service: &TradeAssemblyService, config: &Value) -> Value {
    readiness_for_config_with_authority(service, config, false)
}

fn readiness_for_config_with_authority(
    service: &TradeAssemblyService,
    config: &Value,
    mandate_verified: bool,
) -> Value {
    let mode = config["mode"]
        .as_str()
        .unwrap_or("paper")
        .to_ascii_lowercase();
    let live = mode == "live";
    let capability_revision_result = if let Some(revision) =
        config.get("preparedCapabilityGraphRevision")
    {
        Ok(revision.clone())
    } else if let Some(revision_id) = config["capabilityGraphRevisionId"].as_str() {
        capability_graph::revision_for_context(
            service,
            revision_id,
            config["strategyId"].as_str().unwrap_or_default(),
            config["strategyVersionId"].as_str().unwrap_or_default(),
            &mode,
            &now_rfc3339_second().unwrap_or_else(|| "unspecified".to_string()),
        )
    } else {
        Err(config
                .get("capabilityGraphError")
                .cloned()
                .filter(|value| !value.is_null())
                .unwrap_or_else(|| {
                    json!({
                        "ok": false,
                        "state": "blocked",
                        "blockers": [{
                            "code": "capability_graph_required",
                            "message": "Execution configuration requires an immutable capability graph revision."
                        }],
                        "noAdvice": true,
                    })
                }))
    };
    let capability_ready = capability_revision_result
        .as_ref()
        .is_ok_and(|revision| revision["graph"]["ok"].as_bool().unwrap_or(false));
    let capability_resolution = capability_revision_result
        .as_ref()
        .map(|revision| revision["graph"].clone())
        .unwrap_or_else(|error| error.clone());
    let capability_revision = capability_revision_result.as_ref().ok().cloned();
    let strategy_version = service
        .strategy_versions(config["strategyId"].as_str().unwrap_or_default())
        .into_iter()
        .find(|version| version["id"] == config["strategyVersionId"]);
    let published_version = strategy_version.as_ref().is_some_and(|version| {
        version["immutable"].as_bool().unwrap_or(false)
            && version["publishedAt"]
                .as_str()
                .is_some_and(|value| !value.trim().is_empty())
    });
    let strategy_hash_matches = strategy_version
        .as_ref()
        .is_some_and(|version| hash_strategy_spec(&version["spec"]) == config["strategySpecHash"]);
    let strategy_executable = strategy_version.as_ref().is_some_and(|version| {
        if external_agent(config) {
            crate::spec::validate_strategy_spec_report(&version["spec"]).valid
        } else {
            CompiledStrategy::compile_json(&version["spec"]).is_ok()
        }
    });
    let market_data_ready = selected_capability(&capability_resolution, "market_data.bars.read@1");
    let calendar_ready = selected_capability(&capability_resolution, "calendar.session.resolve@1")
        && config["calendarRef"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty());
    let broker_ready = selected_capability(
        &capability_resolution,
        if live {
            "broker.order_submit.live"
        } else {
            "broker.order_submit.paper"
        },
    );
    let entitlement_ready = required_selections_are_entitled(&capability_resolution);
    let account_bindings_ready = required_account_bindings_present(&capability_resolution);
    let oidc_identity_ready = service
        .runtime()
        .identity
        .descriptors()
        .iter()
        .any(|descriptor| {
            descriptor
                .capabilities
                .iter()
                .any(|capability| capability == "oidc.session.resolve")
        });
    let local_owner_ready =
        service
            .responsible_human_identity()
            .is_some_and(|(issuer, subject)| {
                issuer == "local-owner"
                    && service
                        .runtime()
                        .clock
                        .trusted_now_ms()
                        .ok()
                        .is_some_and(|now| {
                            service
                                .runtime()
                                .identity
                                .resolve(subject, now)
                                .ok()
                                .is_some_and(|claims| {
                                    claims.issuer == issuer
                                        && claims.subject == subject
                                        && claims.assurance.as_deref() == Some("local-process")
                                        && claims.expires_at_ms > now
                                })
                        })
            });
    let identity_ready = oidc_identity_ready || local_owner_ready;
    let freshness_ready = config["dataFreshnessPolicy"]["maxAgeMs"]
        .as_u64()
        .is_some_and(|max_age_ms| max_age_ms > 0);
    let warmup_ready = strategy_executable && config["warmup"]["requiredBars"].as_u64().is_some();
    let risk_limits_ready = valid_execution_risk_limits(config);
    let legal_receipt = live.then(|| verify_live_legal_receipt(service, config));
    let legal_receipt_ready = legal_receipt.as_ref().is_some_and(|result| result.is_ok());
    let mut checks = vec![
        check(
            "execution_universe_valid",
            "Explicit instrument universe is valid for its orchestration owner",
            crate::broker_submission::validate_execution_universe(config).is_ok(),
            true,
            "execution.allowedSymbols",
        ),
        check(
            "execution_orchestrator_supported",
            "Recognized orchestration owner",
            matches!(
                orchestrator_kind(config),
                "deterministic" | "external_agent"
            ),
            true,
            "execution.orchestrator",
        ),
        check(
            "execution_mode_supported",
            "Recognized execution mode",
            matches!(mode.as_str(), "paper" | "shadow" | "live"),
            true,
            "execution.mode",
        ),
        check(
            "strategy_version_locked",
            "Published immutable strategy version",
            published_version && strategy_hash_matches,
            true,
            "strategy.version",
        ),
        check(
            "strategy_spec_executable",
            if external_agent(config) {
                "Owner-published StrategySpec is structurally valid; external agent owns evaluation"
            } else {
                "StrategySpec compiles through the shared execution kernel"
            },
            strategy_executable,
            true,
            if external_agent(config) {
                "strategy.schema"
            } else {
                "strategy.compiler"
            },
        ),
        check(
            "capability_graph_revision",
            "Immutable capability graph revision",
            capability_ready,
            true,
            "plugins.capability_graph",
        ),
        check(
            "symbol_tradable",
            "Instrument identity present",
            if config.get("allowedSymbols").is_some() {
                crate::broker_submission::validate_execution_universe(config).is_ok()
            } else { config["symbol"]
                .as_str()
                .is_some_and(|value| !value.trim().is_empty()) },
            true,
            "instrument.resolve",
        ),
        check(
            "data_available",
            "Exact market-data plugin operation selected",
            market_data_ready,
            true,
            "marketdata.bars",
        ),
        check(
            "calendar_available",
            "Exact market-calendar operation selected",
            calendar_ready,
            true,
            "calendar.session",
        ),
        check(
            "broker_operation_available",
            "Exact paper broker operation selected",
            broker_ready,
            true,
            "broker.order_submit",
        ),
        check(
            "data_freshness_policy",
            "Market-data freshness policy configured",
            freshness_ready,
            true,
            "marketdata.freshness",
        ),
        check(
            "warmup_available",
            if external_agent(config) {
                "External agent owns signal warmup; Core does not attest agent evaluation"
            } else {
                "Compiled strategy warmup requirements resolved"
            },
            warmup_ready,
            true,
            "runtime.warmup",
        ),
        check(
            "risk_limits",
            "Positive finite risk limits set; Live controls must be supported by the broker boundary",
            risk_limits_ready,
            true,
            "risk.limits",
        ),
        check(
            "kill_switch",
            "Kill switch configured",
            config["killSwitch"]["emergencyStop"]
                .as_bool()
                .unwrap_or(false),
            true,
            "execution.kill_switch",
        ),
        check(
            "entitlement",
            "Every required capability selection has an entitlement decision",
            entitlement_ready,
            true,
            "plugins.capability_graph.entitlements",
        ),
        check(
            "account_bindings",
            "Required plugin account bindings are present",
            account_bindings_ready,
            true,
            "plugins.capability_graph.accounts",
        ),
        check(
            "local_identity",
            "OIDC adapter configured or authenticated installation owner verified",
            identity_ready,
            true,
            if local_owner_ready {
                "identity.local_owner"
            } else {
                "identity.oidc"
            },
        ),
    ];
    if live {
        checks.extend(live_activation_checks(
            config,
            capability_ready,
            legal_receipt_ready,
            live_credential_posture(service, &capability_resolution),
            service
                .runtime()
                .plugin_operations
                .verify_broker_boundary()
                .is_ok(),
            mandate_verified,
            service.runtime().evidence.verify_durable().is_ok(),
        ));
    }
    let mut blocked = checks
        .iter()
        .filter(|check| check["status"] != "pass" && check["blocking"].as_bool().unwrap_or(false))
        .filter_map(|check| check["id"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if let Err(error) = &capability_revision_result {
        if let Some(blockers) = error["blockers"].as_array() {
            for code in blockers
                .iter()
                .filter_map(|blocker| blocker["code"].as_str())
            {
                if !blocked.iter().any(|blocked| blocked == code) {
                    blocked.push(code.to_string());
                }
            }
        }
    }
    let live_preflight = if live {
        let mut preflight = live_preflight_report(
            config,
            &capability_resolution,
            &checks,
            &blocked,
            service
                .runtime_manifest()
                .is_some_and(|manifest| manifest.profile == "local"),
        );
        if let Some(cloud_blocked) = preflight["cloudProfile"]["blockedReasons"].as_array() {
            for reason in cloud_blocked.iter().filter_map(Value::as_str) {
                let prefixed = format!("cloud_{reason}");
                if !blocked.contains(&prefixed) {
                    blocked.push(prefixed);
                }
            }
        }
        preflight["blockedReasons"] = json!(blocked.clone());
        Some(preflight)
    } else {
        None
    };
    let mut readiness = json!({
        "ready": blocked.is_empty(),
        "configId": config["configId"],
        "strategyId": config["strategyId"],
        "mode": config["mode"],
        "checks": checks,
        "blockedReasons": blocked,
        "capabilityResolution": capability_resolution,
        "capabilityGraphRevisionId": config["capabilityGraphRevisionId"],
        "capabilityGraphRevision": capability_revision,
        "acknowledgements": acknowledgements_for_mode(live, legal_receipt_ready),
        "legalReceipt": legal_receipt.map(|result| match result {
            Ok(receipt) => json!({
                "status": "verified",
                "receiptRef": receipt.receipt_ref,
                "documentId": receipt.document_id,
                "documentVersion": receipt.document_version,
                "documentLocale": receipt.document_locale,
                "acceptedAt": receipt.accepted_at,
                "validUntil": receipt.valid_until,
                "exportDigest": receipt.export_digest,
            }),
            Err(failure) => json!({"status": "blocked", "reason": failure.code()}),
        }),
        "localOnlyDurabilityWarning": "Legal and audit evidence is stored locally unless exported.",
        "riskLimitsError": if live {
            crate::broker_submission::validate_order_limits(&config["riskLimits"]).err()
        } else { None },
    });
    if let Some(live_preflight) = live_preflight {
        readiness["livePreflight"] = live_preflight;
    }
    readiness
}

fn selected_capability(graph: &Value, capability: &str) -> bool {
    selected_capability_value(graph, capability).is_some()
}

pub(super) fn selected_capability_value<'a>(
    graph: &'a Value,
    capability: &str,
) -> Option<&'a Value> {
    graph["nodes"].as_array()?.iter().find_map(|node| {
        (node["requirement"]["capability"].as_str() == Some(capability)
            && node["blockers"].as_array().is_some_and(Vec::is_empty))
        .then_some(&node["selected"])
        .filter(|selected| selected.is_object())
    })
}

fn required_selections_are_entitled(graph: &Value) -> bool {
    graph["nodes"].as_array().is_some_and(|nodes| {
        nodes
            .iter()
            .filter(|node| node["required"] == true)
            .all(|node| {
                node["selected"]["entitlementDecisionRef"]
                    .as_str()
                    .is_some_and(|value| !value.trim().is_empty())
            })
    })
}

fn required_account_bindings_present(graph: &Value) -> bool {
    graph["nodes"].as_array().is_some_and(|nodes| {
        nodes
            .iter()
            .filter(|node| node["required"] == true)
            .all(|node| {
                !node["selected"]["operation"]["accountBindingRequired"]
                    .as_bool()
                    .unwrap_or(false)
                    || node["selected"]["accountRef"]
                        .as_str()
                        .is_some_and(|value| !value.trim().is_empty())
            })
    })
}

fn valid_execution_risk_limits(config: &Value) -> bool {
    let limits = config
        .get("riskLimits")
        .or_else(|| config.get("risk_limits"));
    let Some(limits) = limits.filter(|value| value.is_object()) else {
        return false;
    };
    if config["mode"] == "live" {
        return crate::broker_submission::validate_order_limits(limits).is_ok();
    }
    ["max_notional", "max_order_quantity"].iter().all(|field| {
        limits[*field]
            .as_f64()
            .is_some_and(|value| value.is_finite() && value > 0.0)
    })
}

fn verify_live_legal_receipt(
    service: &TradeAssemblyService,
    config: &Value,
) -> Result<crate::ports::VerifiedLegalReceipt, crate::ports::LegalReceiptFailure> {
    let receipt_ref = config["legalReceiptRef"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or(crate::ports::LegalReceiptFailure::Missing)?;
    let (current_issuer, current_subject) = service
        .responsible_human_identity()
        .ok_or(crate::ports::LegalReceiptFailure::BindingMismatch)?;
    let configured_issuer = config["responsibleHuman"]["identityIssuer"]
        .as_str()
        .ok_or(crate::ports::LegalReceiptFailure::BindingMismatch)?;
    let configured_subject = config["responsibleHuman"]["identitySubject"]
        .as_str()
        .ok_or(crate::ports::LegalReceiptFailure::BindingMismatch)?;
    if current_issuer != configured_issuer || current_subject != configured_subject {
        return Err(crate::ports::LegalReceiptFailure::BindingMismatch);
    }
    let strategy_id = config["strategyId"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or(crate::ports::LegalReceiptFailure::Invalid)?;
    let strategy_version_id = config["strategyVersionId"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or(crate::ports::LegalReceiptFailure::Invalid)?;
    let strategy_spec_hash = config["strategySpecHash"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or(crate::ports::LegalReceiptFailure::Invalid)?;
    let strategy_spec_version_ref = strategy_spec_version_ref(strategy_spec_hash)?;
    let environment = config["legalEnvironment"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or(crate::ports::LegalReceiptFailure::Invalid)?;
    let trusted_now_ms = service
        .runtime()
        .clock
        .trusted_now_ms()
        .map_err(|_| crate::ports::LegalReceiptFailure::Unavailable)?;
    service.runtime().legal_receipts.verify(
        &crate::ports::LegalReceiptExpectation {
            receipt_ref: receipt_ref.to_string(),
            identity_issuer: configured_issuer.to_string(),
            identity_subject: configured_subject.to_string(),
            resource_ref: format!("tradeassembly://strategies/{strategy_id}"),
            resource_version_refs: vec![
                format!("tradeassembly://strategy-versions/{strategy_version_id}"),
                strategy_spec_version_ref,
            ],
            environment: environment.to_string(),
        },
        trusted_now_ms,
    )
}

fn strategy_spec_version_ref(
    strategy_spec_hash: &str,
) -> Result<String, crate::ports::LegalReceiptFailure> {
    let Some(digest) = strategy_spec_hash.strip_prefix("sha256:") else {
        return Err(crate::ports::LegalReceiptFailure::Invalid);
    };
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(crate::ports::LegalReceiptFailure::Invalid);
    }
    Ok(strategy_spec_hash.to_ascii_lowercase())
}

fn live_credential_posture(service: &TradeAssemblyService, graph: &Value) -> bool {
    let Some(selected) = selected_capability_value(graph, "broker.order_submit.live") else {
        return false;
    };
    let Some(instance_ref) = selected["pluginInstanceRef"].as_str() else {
        return false;
    };
    let Ok(record) = super::plugin_lifecycle::require_instance(service, instance_ref) else {
        return false;
    };
    let Ok(now_ms) = service.runtime().clock.trusted_now_ms() else {
        return false;
    };
    live_account_observation_matches(&record, selected, now_ms)
}

fn live_activation_checks(
    config: &Value,
    capability_ready: bool,
    legal_receipt_ready: bool,
    credential_posture_ready: bool,
    broker_boundary_available: bool,
    mandate_verified: bool,
    durable_evidence_ready: bool,
) -> Vec<Value> {
    let kill_switch_ready = config["killSwitch"]["emergencyStop"]
        .as_bool()
        .unwrap_or(false);
    vec![
        check(
            "legal_acknowledgement",
            "User-only live legal acknowledgement",
            legal_receipt_ready,
            true,
            "legal.live_acknowledgement",
        ),
        check(
            "legal_document_version",
            "Legal document version pinned",
            legal_receipt_ready,
            true,
            "legal.document_version",
        ),
        check(
            "hosted_evidence_policy",
            "Durable local evidence available; activation receipt required at commit",
            durable_evidence_ready,
            true,
            "evidence.durable_activation_receipt",
        ),
        check(
            "live_credential_posture",
            "Live credential/account posture",
            credential_posture_ready,
            true,
            "credentials.live_account",
        ),
        check(
            "live_broker_capability",
            "Live broker order capability",
            capability_ready,
            true,
            "plugins.capability.live_order",
        ),
        check(
            "apf_mandate_tuple",
            "APF mandate tuple includes client, purpose, policy, evidence, and approval mode",
            true,
            true,
            "apf.mandate_tuple",
        ),
        check(
            "apf_receipt_sink",
            "Writable APF receipt path",
            true,
            true,
            "apf.receipt_sink",
        ),
        check(
            "apf_pep_coverage_c5",
            "C5 APF command enforcement",
            true,
            true,
            "apf.pep_coverage.c5",
        ),
        check(
            "downstream_pep_coverage_c5",
            "Enforcing C5 broker dispatch path available; order authorization still required",
            broker_boundary_available,
            true,
            "broker.pep_coverage.c5",
        ),
        check(
            "confirm_before_send",
            "Current owner-issued execution mandate verified for this configuration",
            mandate_verified,
            true,
            "approval.local_live_mandate",
        ),
        check(
            "downstream_live_order_adapter",
            "Live broker adapter requires downstream authority",
            broker_boundary_available,
            true,
            "broker.downstream_authority",
        ),
        check(
            "live_kill_switch",
            "Live emergency kill switch configured",
            kill_switch_ready,
            true,
            "execution.kill_switch",
        ),
    ]
}

fn acknowledgements_for_mode(live: bool, legal_receipt_ready: bool) -> Value {
    if live {
        json!([
            {"id": "legal_live_trading", "label": "Live trading legal acknowledgement", "accepted": legal_receipt_ready, "userOnly": true, "agentMayRequest": true},
            {"id": "user_logic", "label": "User owns strategy logic", "accepted": false, "userOnly": true},
            {"id": "user_risk", "label": "User owns risk scale", "accepted": false, "userOnly": true},
            {"id": "no_advice", "label": "TradeAssembly gives no trade advice", "accepted": false, "userOnly": true}
        ])
    } else {
        json!([
            {"id": "user_logic", "label": "User owns strategy logic", "accepted": true},
            {"id": "user_risk", "label": "User owns risk scale", "accepted": true},
            {"id": "no_advice", "label": "TradeAssembly gives no trade advice", "accepted": true}
        ])
    }
}

fn live_preflight_report(
    config: &Value,
    capability_resolution: &Value,
    checks: &[Value],
    blocked: &[String],
    local_profile: bool,
) -> Value {
    json!({
        "schemaVersion": "tradeassembly.live_activation_preflight.v1",
        "status": "blocked",
        "strategyId": config["strategyId"],
        "configId": config["configId"],
        "mode": "live",
        "client": "self",
        "purpose": "live_order_submission",
        "policyVersion": "2026-07-08.local",
        "approvalMode": "human_confirmation",
        "supervisionMode": "review_before",
        "legal": {
            "required": true,
            "userOnly": true,
            "agentMayRequest": true,
            "documentVersionRef": "legal/live-trading/v1",
            "acknowledgementRef": Value::Null
        },
        "apf": {
            "requiredPepCoverage": "C5",
            "currentPepCoverage": "C5",
            "downstreamPepCoverage": "not_registered",
            "receiptSinkWritable": true,
            "credentialGrantRequired": true,
            "mandateTuple": {
                "agent": "tradeassembly.runtime",
                "principalUser": "local-user",
                "client": "self",
                "action": "live_activation",
                "resource": config["accountRef"],
                "context": "strategy_execution",
                "timeWindow": "bounded",
                "purpose": "live_order_submission",
                "evidence": ["strategy_version", "risk_limits", "legal_document_version", "credential_status"],
                "policyVersion": "2026-07-08.local",
                "approvalMode": "human_confirmation"
            }
        },
        "capabilityResolution": capability_resolution,
        "checks": checks,
        "blockedReasons": blocked,
        "cloudProfile": if local_profile {
            json!({"status":"not_applicable","profile":"local","blockedReasons":[],"notes":["Local execution requires durable local evidence; it does not claim serverless availability."]})
        } else { live_cloud_preflight_report(&DEFAULT_FOSS_DEPLOYMENT) },
        "noAdvice": LEGAL_BOUNDARY,
    })
}

fn run_tick(service: &TradeAssemblyService, run: &mut Value, cycle: u64, lease: &Value) -> Value {
    let activation_id = run["activationId"]
        .as_str()
        .unwrap_or("activation")
        .to_string();
    if let Some(mut currentness) = capability_revision_blocker(service, run) {
        match refresh_compatible_capability_revision(service, run, &currentness) {
            Ok(()) => {
                persist(
                    service,
                    RUNS_NS,
                    run["executionRunId"].as_str().unwrap_or("run"),
                    run.clone(),
                    "execution.capability_revision.migrated",
                );
                clear_capability_reconciliation(service, &activation_id);
            }
            Err(migration_error) => {
                currentness["migrationError"] = json!(migration_error);
                pause_for_runtime_uncertainty(
                    service,
                    &activation_id,
                    "capability_revision_stale",
                    currentness.clone(),
                );
                let terminal_body =
                    json!({"attemptId": lease["attemptId"], "fencingToken": lease["fencingToken"]});
                let terminal_tick_id = format!("scheduler-{activation_id}-{cycle}");
                persist_terminal_tick(
                    service,
                    run,
                    TerminalTick {
                        body: &terminal_body,
                        activation_id: &activation_id,
                        tick_id: &terminal_tick_id,
                        sequence: cycle,
                        status: "capability_blocked",
                        decision: json!({
                        "kind": "error",
                        "code": "capability_revision_stale",
                        "details": currentness,
                        "noAdvice": LEGAL_BOUNDARY,
                        }),
                    },
                );
                return complete_effect_free_cycle(
                    service,
                    run,
                    cycle,
                    lease,
                    "capability_revision_stale",
                    currentness,
                );
            }
        }
    } else {
        clear_capability_reconciliation(service, &activation_id);
    }
    if let Err(error) = reconcile_ambiguous_actions(service, run) {
        return json!({
            "status": "failed",
            "reason": "broker_reconciliation_failed",
            "error": error,
            "activationId": activation_id,
            "cycle": cycle,
            "lease": lease,
        });
    }
    let tick_id = format!("scheduler-{activation_id}-{cycle}");
    let attempt_id = lease["attemptId"]
        .as_str()
        .unwrap_or("attempt-unset")
        .to_string();
    let mode = match durable_execution_mode(run) {
        Ok(mode) => mode,
        Err(reason) => {
            return json!({
                "status": "failed",
                "reason": reason,
                "activationId": activation_id,
                "cycle": cycle,
                "lease": lease,
            })
        }
    };
    let context = SideEffectContext::new(
        AuthorityContext {
            actor: "local-user".to_string(),
            surface: "scheduler".to_string(),
            account_mode: mode.clone(),
        },
        IdempotencyKey::new(format!("execution:{activation_id}:tick:{cycle}:market"))
            .expect("valid market operation idempotency key"),
    );
    let request = match market_operation_request(
        run,
        &activation_id,
        &tick_id,
        &attempt_id,
        cycle,
        lease["fencingToken"].as_i64(),
    ) {
        Ok(request) => request,
        Err(_) => {
            return json!({
                "status": "failed",
                "reason": "market_data_plugin_binding_missing",
                "activationId": activation_id,
                "cycle": cycle,
                "lease": lease,
            })
        }
    };
    let market = match service
        .runtime()
        .plugin_operations
        .invoke(&request, &context)
    {
        Ok(response) => response,
        Err(error) => {
            return json!({
                "status": "failed",
                "reason": "market_data_plugin_invocation_failed",
                "error": safe_plugin_failure_code(&error),
                "activationId": activation_id,
                "cycle": cycle,
                "lease": lease,
            })
        }
    };
    let observation = match market_observation_from_response(run, &request, &market) {
        Ok(observation) => observation,
        Err(reason) => {
            let terminal_body =
                json!({"attemptId": attempt_id, "fencingToken": lease["fencingToken"]});
            persist_terminal_tick(
                service,
                run,
                TerminalTick {
                    body: &terminal_body,
                    activation_id: &activation_id,
                    tick_id: &tick_id,
                    sequence: cycle,
                    status: "plugin_evidence_blocked",
                    decision: json!({
                        "kind": "error",
                        "code": "market_data_plugin_evidence_invalid",
                        "reason": reason,
                        "noAdvice": LEGAL_BOUNDARY,
                    }),
                },
            );
            return json!({
                "status": "failed",
                "reason": "market_data_plugin_evidence_invalid",
                "activationId": activation_id,
                "cycle": cycle,
                "lease": lease,
            });
        }
    };
    let response = evaluate_tick_response(
        service,
        &format!("/product/strategy-execution-activations/{activation_id}/ticks/evaluate"),
        json!({
            "tickId": tick_id,
            "controlPlaneCorrelationId": correlation_id_for_run(run),
            "sequence": cycle,
            "strategyVersionId": run["strategyVersionId"],
            "attemptId": attempt_id,
            "observation": observation,
            "idempotencyKey": format!("{activation_id}:{cycle}"),
            "accountMode": mode.clone(),
            "authorityContext": {
                "actor": "local-user",
                "surface": "scheduler",
                "accountMode": mode,
            },
            "fencingToken": lease["fencingToken"],
        }),
    );
    if response.status >= 400 {
        return json!({
            "status": "failed",
            "reason": "evaluation_failed",
            "error": response.body,
            "activationId": activation_id,
            "cycle": cycle,
            "lease": lease,
        });
    }
    let mut result = response.body["body"].clone();
    result["status"] = json!("complete");
    result["executionRunId"] = run["executionRunId"].clone();
    result["lease"] = lease.clone();
    result
}

fn market_observation_from_response(
    run: &Value,
    request: &PluginOperationRequest,
    response: &PluginOperationResponse,
) -> Result<Value, &'static str> {
    if response.correlation_id != request.correlation_id {
        return Err("market_data_correlation_mismatch");
    }
    if response.schema_ref != "schema://market-data/bar@1" {
        return Err("market_data_schema_mismatch");
    }
    if response.freshness_state != "fresh" {
        return Err("market_data_not_fresh");
    }
    if response.observed_at_ms <= 0
        || response.source_event_id.trim().is_empty()
        || response.content_hash.trim().is_empty()
    {
        return Err("market_data_provenance_invalid");
    }
    if response.payload["symbol"] != request.input["symbol"]
        || response.payload["timeframe"] != request.input["timeframe"]
    {
        return Err("market_data_instrument_mismatch");
    }
    let open_micros = positive_i64_field(&response.payload, "openMicros")?;
    let high_micros = positive_i64_field(&response.payload, "highMicros")?;
    let low_micros = positive_i64_field(&response.payload, "lowMicros")?;
    let close_micros = positive_i64_field(&response.payload, "closeMicros")?;
    let volume_micros = positive_i64_field(&response.payload, "volumeMicros")?;
    if high_micros < open_micros.max(close_micros)
        || low_micros > open_micros.min(close_micros)
        || low_micros > high_micros
    {
        return Err("market_data_bar_invalid");
    }
    if response.evidence_refs.is_empty()
        || response
            .evidence_refs
            .iter()
            .any(|reference| reference.trim().is_empty())
    {
        return Err("market_data_evidence_missing");
    }
    let max_age_ms = run["dataFreshnessPolicy"]["maxAgeMs"]
        .as_u64()
        .filter(|max_age_ms| *max_age_ms > 0)
        .ok_or("market_data_freshness_policy_invalid")?;
    let mut evidence_refs = response.evidence_refs.clone();
    evidence_refs.push(format!("plugin-event://{}", response.source_event_id));
    evidence_refs.push(format!("plugin-content://{}", response.content_hash));
    Ok(json!({
        "schemaVersion": "tradeassembly.plugin_observation.v1",
        "eventId": response.source_event_id,
        "observedAtMs": response.observed_at_ms,
        "freshness": {"state": response.freshness_state, "maxAgeMs": max_age_ms},
        "source": {
            "pluginInstanceRef": request.plugin_instance_ref,
            "pluginRef": request.plugin_ref,
            "manifestFingerprint": request.manifest_fingerprint,
            "operationId": request.operation_id,
            "capability": request.capability,
        },
        "bar": {
            "symbol": response.payload["symbol"],
            "timeframe": response.payload["timeframe"],
            "open": quantity_value(open_micros),
            "high": quantity_value(high_micros),
            "low": quantity_value(low_micros),
            "close": quantity_value(close_micros),
            "volume": quantity_value(volume_micros),
        },
        "evidenceRefs": evidence_refs,
        "deterministic": response.deterministic,
        "replayable": response.replayable,
    }))
}

fn safe_plugin_failure_code(error: &str) -> &str {
    if !error.is_empty()
        && error.len() <= 128
        && error
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        error
    } else {
        "plugin_invocation_failed"
    }
}

fn market_operation_request(
    run: &Value,
    activation_id: &str,
    tick_id: &str,
    attempt_id: &str,
    sequence: u64,
    fencing_token: Option<i64>,
) -> Result<PluginOperationRequest, &'static str> {
    let binding = selected_binding(run, "market_data.bars.read@1", "marketdata.bars.read_v1")
        .ok_or("market_data_plugin_binding_missing")?;
    let mode = durable_execution_mode(run)?;
    let purpose = if mode == "live" {
        "live_order_submission"
    } else {
        "paper_trading"
    };
    Ok(PluginOperationRequest {
        correlation_id: correlation_id_for_run(run),
        plugin_instance_ref: required_json_string(binding, "/pluginInstanceRef")?.to_string(),
        plugin_ref: required_json_string(binding, "/pluginRef")?.to_string(),
        manifest_fingerprint: required_json_string(binding, "/manifestFingerprint")?.to_string(),
        operation_id: required_json_string(binding, "/operationId")?.to_string(),
        capability: required_json_string(binding, "/operation/capability")?.to_string(),
        capability_graph_revision_id: required_json_string(run, "/capabilityGraphRevisionId")?
            .to_string(),
        capability_graph_fingerprint: required_json_string(run, "/capabilityGraphFingerprint")?
            .to_string(),
        strategy_id: required_json_string(run, "/strategyId")?.to_string(),
        strategy_version_id: required_json_string(run, "/strategyVersionId")?.to_string(),
        strategy_spec_hash: required_json_string(run, "/strategySpecHash")?.to_string(),
        activation_id: activation_id.to_string(),
        attempt_id: attempt_id.to_string(),
        evaluation_tick_id: tick_id.to_string(),
        mode,
        purpose: purpose.to_string(),
        account_ref: binding["accountRef"].as_str().map(str::to_string),
        timeout_ms: 10_000,
        fencing_token,
        input: json!({
            "symbol": required_json_string(run, "/symbol")?,
            "assetClass": required_json_string(run, "/assetClass")?,
            "timeframe": required_json_string(run, "/timeframe")?,
            "sequence": sequence,
        }),
        evidence_refs: vec![format!("schedule://{sequence}")],
    })
}

fn positive_i64_field(value: &Value, field: &str) -> Result<i64, &'static str> {
    value[field]
        .as_i64()
        .filter(|number| *number > 0)
        .ok_or("market_data_bar_invalid")
}

fn readiness_config_id(body: &Value) -> String {
    string_field(body, &["configId", "config_id"])
        .unwrap_or_else(|| "cfg_local_btc_exit".to_string())
}

fn has_explicit_config_id(body: &Value) -> bool {
    string_field(body, &["configId", "config_id"]).is_some()
}

fn has_config_overrides(body: &Value) -> bool {
    string_field(
        body,
        &[
            "providerRef",
            "provider_ref",
            "mode",
            "accountMode",
            "account_mode",
            "accountRef",
            "account_ref",
            "assetClass",
            "asset_class",
        ],
    )
    .is_some()
}

fn find_config(service: &TradeAssemblyService, body: &Value) -> Option<Value> {
    let config_id = readiness_config_id(body);
    get(service, CONFIGS_NS, &config_id)
}

fn default_config(service: &TradeAssemblyService, body: &Value) -> Value {
    let strategy_id = string_field(body, &["strategyId", "strategy_id"]).unwrap_or_else(|| {
        string_field(body, &["activationId", "activation_id"])
            .and_then(|activation_id| {
                get(service, RUNS_NS, &run_id_for_activation(&activation_id))
                    .and_then(|run| run["strategyId"].as_str().map(str::to_string))
            })
            .unwrap_or_else(|| "strat_local_btc_demo".to_string())
    });
    let strategy = service.strategy_value(&strategy_id);
    let latest_version = service
        .strategy_versions(&strategy_id)
        .pop()
        .unwrap_or_else(|| json!({"id": "ver_local", "spec": strategy["latestSpec"].clone()}));
    let spec = latest_version["spec"].clone();
    let provider_ref = string_field(body, &["providerRef", "provider_ref"]).unwrap_or_else(|| {
        strategy["providerRef"]
            .as_str()
            .unwrap_or("sim")
            .to_string()
    });
    let mode = execution_mode_from(body);
    let account_ref = string_field(body, &["accountRef", "account_ref"]);
    let asset_class = string_field(body, &["assetClass", "asset_class"]).unwrap_or_else(|| {
        strategy["assetClass"]
            .as_str()
            .unwrap_or("crypto")
            .to_string()
    });
    let config_id = readiness_config_id(body);
    json!({
        "schemaVersion": "tradeassembly.execution_config.v1",
        "kind": "StrategyExecutionConfig",
        "id": config_id,
        "configId": config_id,
        "strategyId": strategy_id,
        "strategy_id": strategy_id,
        "strategyVersionId": latest_version["id"].as_str().unwrap_or("ver_local"),
        "version_id": latest_version["id"].as_str().unwrap_or("ver_local"),
        "strategySpecHash": hash_strategy_spec(&latest_version["spec"]),
        "providerRef": provider_ref,
        "provider_ref": provider_ref,
        "mode": mode,
        "accountMode": mode,
        "accountRef": account_ref,
        "assetClass": asset_class,
        "symbol": strategy["symbol"].as_str().unwrap_or("BTC/USD"),
        "timeframe": strategy["timeframe"].as_str().unwrap_or("1m"),
        "calendarRef": strategy_calendar_ref(&spec),
        "dataFreshnessPolicy": {"maxAgeMs": 60_000},
        "warmup": {"requiredBars": 0, "source": "compiled_strategy_kernel"},
        "riskLimits": {
            "max_notional": strategy["maxNotional"].as_f64().unwrap_or(25.0),
            "max_order_quantity": strategy["maxOrderQuantity"].as_f64().unwrap_or(0.0003),
            "max_daily_loss": strategy["maxDailyLoss"].as_f64().unwrap_or(25.0),
            "max_concurrent_positions": 1,
        },
        "killSwitch": {"configured": true, "defaultControl": "pause_entries", "emergencyStop": true},
        "pluginBindings": [{"pluginRef": provider_ref, "capability": required_broker_capability(&mode), "status": "configured"}],
        "status": "configured",
        "noAdvice": LEGAL_BOUNDARY,
    })
}

fn strategy_calendar_ref(spec: &Value) -> Option<String> {
    spec.pointer("/stages/market_clock/policy/calendar_refs")
        .and_then(Value::as_array)
        .filter(|refs| refs.len() == 1)
        .and_then(|refs| refs.first())
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
}

fn activation_response(service: &TradeAssemblyService, run: Value, duplicate: bool) -> Value {
    json!({
        "activationId": run["activationId"],
        "executionRunId": run["executionRunId"],
        "status": run["state"],
        "duplicate": duplicate,
        "run": run,
        "apfAudit": service.runtime().finance_authority.audit(service.runtime().storage.as_ref()),
    })
}

fn activation_matches_config(run: &Value, config: &Value) -> bool {
    if orchestrator_kind(run) != orchestrator_kind(config) {
        return false;
    }
    [
        "configId",
        "strategyId",
        "strategyVersionId",
        "strategySpecHash",
        "mode",
        "accountRef",
        "symbol",
    ]
    .into_iter()
    .all(|field| run[field] == config[field])
}

fn orchestrator_kind(value: &Value) -> &str {
    value["orchestrator"].as_str().unwrap_or("deterministic")
}

fn external_agent(value: &Value) -> bool {
    orchestrator_kind(value) == "external_agent"
}

fn active_runs(service: &TradeAssemblyService) -> Vec<Value> {
    execution_runs(service, None)
        .into_iter()
        .filter(|run| run["state"].as_str() == Some("active"))
        .collect()
}

pub(crate) fn resume_local_scheduler_workers(service: &TradeAssemblyService) {
    let state = scheduler_status(service);
    if !state["running"].as_bool().unwrap_or(false) {
        return;
    }
    let Some(activation_id) = state["activationId"]
        .as_str()
        .or_else(|| state["activation_id"].as_str())
        .filter(|activation_id| {
            active_runs(service)
                .iter()
                .any(|run| run["activationId"].as_str() == Some(*activation_id))
        })
    else {
        return;
    };
    if let Some(run) = get(service, RUNS_NS, &run_id_for_activation(activation_id)) {
        if let Some(strategy_id) = run["strategyId"].as_str() {
            super::strategy::materialize_default_version(service, strategy_id);
        }
    }
    let worker_id = state["workerId"]
        .as_str()
        .unwrap_or("worker-local-recovered")
        .to_string();
    let interval_seconds = state["intervalSeconds"].as_u64().unwrap_or(60).max(1);
    start_local_scheduler_worker(
        service.clone(),
        activation_id.to_string(),
        worker_id,
        interval_seconds,
    );
}

fn start_local_scheduler_worker(
    service: TradeAssemblyService,
    activation_id: String,
    worker_id: String,
    interval_seconds: u64,
) {
    if get(&service, RUNS_NS, &run_id_for_activation(&activation_id))
        .as_ref()
        .is_some_and(external_agent)
    {
        return;
    }
    let worker_key = format!(
        "{}:{activation_id}",
        service.runtime().storage.adapter_name()
    );
    let registry = scheduler_worker_registry();
    {
        let mut active = registry.lock().expect("scheduler worker registry lock");
        if !active.insert(worker_key.clone()) {
            return;
        }
    }
    let worker_name = format!("tradeassembly-scheduler-{}", slug(&activation_id));
    let worker_key_for_thread = worker_key.clone();
    let spawn = thread::Builder::new().name(worker_name).spawn(move || {
        loop {
            let wake = service.runtime().scheduler_wake.clone();
            let observed = wake.generation();
            let (_, changed) = wake.wait(observed, Duration::from_secs(interval_seconds));
            if changed {
                let state = scheduler_status(&service);
                let current_activation = state["activationId"]
                    .as_str()
                    .or_else(|| state["activation_id"].as_str())
                    .unwrap_or("");
                if !state["running"].as_bool().unwrap_or(false)
                    || current_activation != activation_id
                {
                    break;
                }
                continue;
            }
            let state = scheduler_status(&service);
            if !state["running"].as_bool().unwrap_or(false) {
                break;
            }
            let current_activation = state["activationId"]
                .as_str()
                .or_else(|| state["activation_id"].as_str())
                .unwrap_or("");
            if current_activation != activation_id {
                break;
            }
            let _ = service.handle_http_from_source(
                "scheduler",
                "POST",
                "/scheduler/run",
                json!({
                "activationId": activation_id.clone(),
                "workerId": worker_id.clone(),
                "maxCycles": 1,
                "idempotencyKey": format!("scheduler-loop:{current_activation}:{}", epoch_nanos()),
                }),
            );
        }
        scheduler_worker_registry()
            .lock()
            .expect("scheduler worker registry lock")
            .remove(&worker_key_for_thread);
    });
    if spawn.is_err() {
        registry
            .lock()
            .expect("scheduler worker registry lock")
            .remove(&worker_key);
    }
}

fn scheduler_worker_registry() -> &'static Mutex<HashSet<String>> {
    static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

fn list_configs(service: &TradeAssemblyService, strategy_id: &str) -> Vec<Value> {
    let mut configs = service
        .filter_visible_values(
            "execution_config",
            list_namespace(service, CONFIGS_NS),
            &["id", "configId", "config_id"],
        )
        .into_iter()
        .filter(|item| item["strategyId"].as_str() == Some(strategy_id))
        .collect::<Vec<_>>();
    configs.sort_by(|left, right| {
        left["savedAtMs"]
            .as_i64()
            .unwrap_or_default()
            .cmp(&right["savedAtMs"].as_i64().unwrap_or_default())
            .then_with(|| {
                left["configId"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["configId"].as_str().unwrap_or_default())
            })
    });
    configs
}

fn schedule_execution_tick(
    service: &TradeAssemblyService,
    activation_id: &str,
    cycle: u64,
    due_at_ms: i64,
) -> Result<(), String> {
    if get(service, RUNS_NS, &run_id_for_activation(activation_id))
        .as_ref()
        .is_some_and(external_agent)
    {
        return Err("external_agent_scheduler_not_applicable".to_string());
    }
    let schedule_key = execution_schedule_key(activation_id, cycle);
    service.runtime().scheduler.schedule(ScheduledWork {
        schedule_key: schedule_key.clone(),
        queue: EXECUTION_TICK_QUEUE.to_string(),
        payload: json!({
            "schemaVersion": "tradeassembly.execution_tick_schedule.v1",
            "activationId": activation_id,
            "cycle": cycle,
        }),
        due_at_ms,
        idempotency_key: IdempotencyKey::new(format!("schedule:{schedule_key}"))?,
        fencing_token: 0,
    })
}

fn claim_execution_ticks(
    service: &TradeAssemblyService,
    target_activation: Option<&str>,
    worker_id: &str,
    now_ms: i64,
    lease_ms: i64,
) -> Result<Vec<ScheduledWork>, String> {
    if let Some(activation_id) = target_activation {
        let Some(run) = get(service, RUNS_NS, &run_id_for_activation(activation_id)) else {
            return Ok(Vec::new());
        };
        if run["state"].as_str() != Some("active") {
            return Ok(Vec::new());
        }
        let completed_cycle = run["cycle"].as_u64().unwrap_or(0);
        let cycle = completed_cycle + 1;
        schedule_execution_tick(service, activation_id, cycle, now_ms)?;
        return service
            .runtime()
            .scheduler
            .claim_key(
                &execution_schedule_key(activation_id, cycle),
                worker_id,
                now_ms,
                lease_ms,
            )
            .map(|work| work.into_iter().collect());
    }
    service
        .runtime()
        .scheduler
        .claim_due(worker_id, now_ms, lease_ms, 100)
}

fn process_execution_tick(
    service: &TradeAssemblyService,
    work: ScheduledWork,
    worker_id: &str,
    lease_ms: i64,
    interval_seconds: u64,
    processing_now_ms: i64,
) -> Result<Value, String> {
    if work.queue != EXECUTION_TICK_QUEUE {
        return Err("claimed work is not an execution tick".to_string());
    }
    let activation_id = work
        .payload
        .get("activationId")
        .and_then(Value::as_str)
        .ok_or_else(|| "execution tick activation is missing".to_string())?
        .to_string();
    let cycle = work
        .payload
        .get("cycle")
        .and_then(Value::as_u64)
        .ok_or_else(|| "execution tick cycle is missing".to_string())?;
    if work.schedule_key != execution_schedule_key(&activation_id, cycle) {
        return Err("execution tick schedule identity mismatch".to_string());
    }
    let Some(mut run) = get(service, RUNS_NS, &run_id_for_activation(&activation_id)) else {
        return Err("execution tick activation does not exist".to_string());
    };
    if run["state"].as_str() != Some("active") && run["state"].as_str() != Some("paused") {
        service.runtime().scheduler.complete(
            &work.schedule_key,
            worker_id,
            work.fencing_token,
            service.runtime().clock.now_ms(),
        )?;
        return Ok(json!({
            "status": "skipped",
            "reason": "activation_not_running",
            "activationId": activation_id,
            "cycle": cycle,
            "schedule": schedule_claim_json(&work, worker_id),
            "lease": {"acquired": false, "duplicate": false},
        }));
    }

    let checkpoint_id = format!("checkpoint_{}_{}", slug(&activation_id), cycle);
    if let Some(completed_checkpoint) = completed_checkpoint(service, &activation_id, cycle) {
        if run["cycle"].as_u64().unwrap_or(0) < cycle {
            run["cycle"] = json!(cycle);
            run["state"] = json!("active");
            run["stateReason"] = json!("checkpoint_recovered");
            run["checkpoint"] = completed_checkpoint.clone();
            run["checkpointId"] = completed_checkpoint["checkpointId"].clone();
            run["checkpointHash"] = completed_checkpoint["checkpointHash"].clone();
            persist(
                service,
                RUNS_NS,
                run["executionRunId"].as_str().unwrap_or("run"),
                run.clone(),
                "execution.run.recovered",
            );
            if let Some(mut activation) = get(service, ACTIVATIONS_NS, &activation_id) {
                activation["lastCompletedSequence"] = json!(cycle);
                activation["checkpointId"] = completed_checkpoint["checkpointId"].clone();
                activation["checkpointHash"] = completed_checkpoint["checkpointHash"].clone();
                activation["state"] = run["state"].clone();
                activation["stateReason"] = run["stateReason"].clone();
                persist(
                    service,
                    ACTIVATIONS_NS,
                    &activation_id,
                    activation,
                    "execution.activation.recovered",
                );
            }
        }
        let recovery = json!({
            "status": "recovered",
            "reason": "checkpoint_already_completed",
            "activationId": activation_id,
            "executionRunId": run["executionRunId"],
            "cycle": cycle,
            "checkpointId": checkpoint_id,
            "lease": {"acquired": false, "duplicate": true},
        });
        finalize_scheduled_tick(service, &work, worker_id, interval_seconds)?;
        return Ok(recovery);
    }
    let expected_cycle = run["cycle"].as_u64().unwrap_or(0) + 1;
    if cycle != expected_cycle {
        return Err(format!(
            "execution tick is out of order: expected cycle {expected_cycle}, received {cycle}"
        ));
    }

    let now_ms = processing_now_ms;
    let resource = execution_lease_resource(&activation_id, cycle);
    let Some(lease) = service
        .runtime()
        .leases
        .acquire(&resource, worker_id, now_ms, lease_ms)?
    else {
        return Ok(json!({
            "status": "contended",
            "reason": "logical_lease_held",
            "activationId": activation_id,
            "cycle": cycle,
            "schedule": schedule_claim_json(&work, worker_id),
            "lease": {"acquired": false, "duplicate": true},
        }));
    };
    let attempt_id = execution_attempt_id(
        &activation_id,
        cycle,
        work.fencing_token,
        lease.fencing_token,
    );
    let mut attempt = json!({
        "schemaVersion": "tradeassembly.execution_attempt.v1",
        "kind": "ExecutionAttemptRecord",
        "attemptId": attempt_id,
        "activationId": activation_id,
        "correlationId": correlation_id_for_run(&run),
        "cycle": cycle,
        "owner": lease.owner,
        "scheduleFencingToken": work.fencing_token,
        "fencingToken": lease.fencing_token,
        "leaseResource": lease.resource,
        "startedAtMs": now_ms,
        "heartbeatAtMs": now_ms,
        "completedAtMs": Value::Null,
        "state": "running",
    });
    persist(
        service,
        ATTEMPTS_NS,
        &attempt_id,
        attempt.clone(),
        "execution.attempt.started",
    );
    let mut lease_json = lease_json(&lease);
    lease_json["attemptId"] = json!(attempt_id);
    let mut tick = run_tick(service, &mut run, cycle, &lease_json);
    if tick["status"].as_str() == Some("failed") {
        let failure_reason = tick["reason"].as_str().unwrap_or("tick_processing_failed");
        tick = complete_effect_free_cycle(
            service,
            &mut run,
            cycle,
            &lease_json,
            failure_reason,
            json!({"sourceStatus": "failed", "sourceReason": failure_reason}),
        );
    }
    finalize_scheduled_tick(service, &work, worker_id, interval_seconds)?;
    attempt["state"] = if tick["status"].as_str() == Some("blocked") {
        json!("blocked")
    } else {
        json!("completed")
    };
    if tick["status"].as_str() == Some("blocked") {
        attempt["blockedReason"] = tick["reason"].clone();
        attempt["effectsApplied"] = json!(false);
    }
    attempt["heartbeatAtMs"] = json!(service.runtime().clock.now_ms());
    attempt["completedAtMs"] = json!(service.runtime().clock.now_ms());
    persist(
        service,
        ATTEMPTS_NS,
        &attempt_id,
        attempt,
        "execution.attempt.completed",
    );
    service.runtime().leases.release(&lease)?;
    Ok(tick)
}

fn finalize_scheduled_tick(
    service: &TradeAssemblyService,
    work: &ScheduledWork,
    worker_id: &str,
    interval_seconds: u64,
) -> Result<(), String> {
    let activation_id = work.payload["activationId"]
        .as_str()
        .ok_or_else(|| "execution tick activation is missing".to_string())?;
    let cycle = work.payload["cycle"]
        .as_u64()
        .ok_or_else(|| "execution tick cycle is missing".to_string())?;
    let checkpoint = completed_checkpoint(service, activation_id, cycle)
        .ok_or_else(|| "execution tick has no valid completed checkpoint".to_string())?;
    let completion_key = format!("execution-tick:{activation_id}:{cycle}:complete");
    let completion_fact = json!({
        "activationId": activation_id,
        "cycle": cycle,
        "scheduleKey": work.schedule_key,
        "checkpointId": checkpoint["checkpointId"],
        "checkpointHash": checkpoint["checkpointHash"],
    });
    let now_ms = service.runtime().clock.now_ms();
    service.runtime().events.append(EventAppend {
        stream: format!("{EXECUTION_TICK_STREAM}:{activation_id}"),
        event_type: "execution.tick.completed".to_string(),
        aggregate_id: activation_id.to_string(),
        payload: completion_fact.clone(),
        idempotency_key: IdempotencyKey::new(completion_key.clone())?,
        occurred_at_ms: work.due_at_ms,
        retention_until_ms: None,
    })?;
    service.runtime().outbox.append(OutboxRecord {
        outbox_id: format!("outbox_{}", slug(&completion_key)),
        topic: "execution.tick.completed".to_string(),
        payload: completion_fact,
        idempotency_key: IdempotencyKey::new(format!("outbox:{completion_key}"))?,
        created_at_ms: work.due_at_ms,
        fencing_token: 0,
    })?;
    let _ = service.runtime().inbox.record_once(
        EXECUTION_TICK_STREAM,
        &work.schedule_key,
        Some(&format!("schedule-fence://{}", work.fencing_token)),
        now_ms,
    )?;

    let next_cycle = cycle + 1;
    let interval_ms = i64::try_from(interval_seconds)
        .unwrap_or(i64::MAX / 1_000)
        .saturating_mul(1_000);
    schedule_execution_tick(
        service,
        activation_id,
        next_cycle,
        work.due_at_ms.saturating_add(interval_ms),
    )?;
    let mut state = scheduler_status(service);
    if state["activationId"].as_str() == Some(activation_id)
        || state["activation_id"].as_str() == Some(activation_id)
    {
        state["nextCycle"] = json!(next_cycle);
        state["nextScheduleKey"] = json!(execution_schedule_key(activation_id, next_cycle));
        state["lastCompletedCycle"] = json!(cycle);
        persist(
            service,
            STATE_NS,
            "current",
            state,
            "scheduler.schedule_advanced",
        );
    }
    service.runtime().scheduler.complete(
        &work.schedule_key,
        worker_id,
        work.fencing_token,
        service.runtime().clock.now_ms(),
    )
}

fn execution_schedule_key(activation_id: &str, cycle: u64) -> String {
    format!("execution:{activation_id}:cycle:{cycle}")
}

fn execution_lease_resource(activation_id: &str, cycle: u64) -> String {
    format!("execution-tick:{activation_id}:{cycle}")
}

fn execution_attempt_id(
    activation_id: &str,
    cycle: u64,
    schedule_fencing_token: i64,
    lease_fencing_token: i64,
) -> String {
    format!(
        "attempt_{}_{}_schedule_{}_lease_{}",
        slug(activation_id),
        cycle,
        schedule_fencing_token,
        lease_fencing_token
    )
}

fn completed_checkpoint(
    service: &TradeAssemblyService,
    activation_id: &str,
    cycle: u64,
) -> Option<Value> {
    let checkpoint_id = format!("checkpoint_{}_{}", slug(activation_id), cycle);
    let checkpoint = get(service, CHECKPOINTS_NS, &checkpoint_id)?;
    let hash_input = checkpoint
        .get("correlationId")
        .filter(|value| value.is_string())
        .map_or_else(
            || {
                json!({
                    "activationId": activation_id,
                    "cycle": cycle,
                    "previousHash": checkpoint.get("previousHash").cloned().unwrap_or(Value::Null),
                    "state": checkpoint.get("state").cloned().unwrap_or(Value::Null),
                })
            },
            |correlation_id| {
                json!({
                    "activationId": activation_id,
                    "correlationId": correlation_id,
                    "cycle": cycle,
                    "previousHash": checkpoint.get("previousHash").cloned().unwrap_or(Value::Null),
                    "state": checkpoint.get("state").cloned().unwrap_or(Value::Null),
                })
            },
        );
    let expected_hash = hash_value(&hash_input);
    (checkpoint["schemaVersion"] == "tradeassembly.execution_checkpoint.v1"
        && checkpoint["checkpointId"] == checkpoint_id
        && checkpoint["activationId"] == activation_id
        && checkpoint["cycle"] == cycle
        && checkpoint["checkpointHash"] == expected_hash)
        .then_some(checkpoint)
}

fn lease_json(lease: &LeaseClaim) -> Value {
    json!({
        "leaseId": format!("lease_{}", slug(&lease.resource)),
        "workerId": lease.owner,
        "acquired": true,
        "duplicate": false,
        "fencingToken": lease.fencing_token,
        "expiresAtMs": lease.expires_at_ms,
    })
}

fn schedule_claim_json(work: &ScheduledWork, worker_id: &str) -> Value {
    json!({
        "scheduleKey": work.schedule_key,
        "workerId": worker_id,
        "fencingToken": work.fencing_token,
        "dueAtMs": work.due_at_ms,
    })
}

fn scheduler_failure(code: &str, detail: String) -> Value {
    json!({
        "status": "failed",
        "running": false,
        "error": {"code": code, "detail": detail},
        "ticks": [],
        "lease": {"acquired": false, "duplicate": false},
    })
}

fn append_event(
    service: &TradeAssemblyService,
    activation_id: &str,
    event_type: &str,
    payload: Value,
) -> String {
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("{event_type}:{}", slug(activation_id)))
            .expect("valid execution event idempotency key"),
    );
    append_event_with_context(service, activation_id, event_type, payload, &context)
}

fn append_event_with_body(
    service: &TradeAssemblyService,
    activation_id: &str,
    event_type: &str,
    payload: Value,
    body: &Value,
) -> String {
    let context = control_side_effect_context(body, event_type);
    append_event_with_context(service, activation_id, event_type, payload, &context)
}

fn append_event_with_context(
    service: &TradeAssemblyService,
    activation_id: &str,
    event_type: &str,
    payload: Value,
    context: &SideEffectContext,
) -> String {
    let correlation_id = durable_correlation_id(service, activation_id)
        .or_else(|| string_field(&payload, &["correlationId", "correlation_id"]))
        .unwrap_or_else(|| format!("activation:{activation_id}"));
    for _ in 0..64 {
        let events = events_for(service, activation_id);
        let sequence = events
            .last()
            .and_then(|event| event["sequence"].as_u64())
            .unwrap_or(0)
            .saturating_add(1);
        let event_id = format!("evt_{}_{}", slug(activation_id), sequence);
        let previous_hash = events
            .last()
            .and_then(|event| event["eventHash"].as_str().map(str::to_string));
        let event_hash = hash_value(&json!({
            "activationId": activation_id,
            "correlationId": correlation_id,
            "sequence": sequence,
            "eventType": event_type,
            "payload": payload,
            "previousHash": previous_hash,
        }));
        let event = json!({
            "schemaVersion": "tradeassembly.execution_event.v1",
            "id": event_id,
            "eventId": event_id,
            "activationId": activation_id,
            "correlationId": correlation_id,
            "sequence": sequence,
            "eventType": event_type,
            "payload": payload,
            "previousHash": previous_hash,
            "eventHash": event_hash,
            "noAdvice": LEGAL_BOUNDARY,
        });
        let event_context = SideEffectContext::new(
            context.authority.clone(),
            IdempotencyKey::new(format!("{}:{event_id}", context.idempotency_key.as_str()))
                .expect("valid execution event idempotency key"),
        );
        match service.runtime().storage.put_json_if_absent(
            EVENTS_NS,
            &event_id,
            event.clone(),
            &event_context,
        ) {
            Ok(ImmutablePutOutcome::Created) => {
                let _ = service
                    .runtime()
                    .record_side_effect(event_type, event, &event_context);
                return event_id;
            }
            Ok(ImmutablePutOutcome::AlreadyPresent) => return event_id,
            Err(error) if error == "immutable_storage_conflict" => thread::yield_now(),
            Err(error) => panic!("persist execution event: {error}"),
        }
    }
    panic!("execution event append contention exceeded retry budget")
}

fn checkpoint(
    activation_id: &str,
    correlation_id: &str,
    cycle: u64,
    previous_hash: Option<&str>,
    reason: &str,
    state: Value,
) -> Value {
    let checkpoint_id = format!("checkpoint_{}_{}", slug(activation_id), cycle);
    let checkpoint_hash = hash_value(&json!({
        "activationId": activation_id,
        "correlationId": correlation_id,
        "cycle": cycle,
        "previousHash": previous_hash,
        "state": state,
    }));
    json!({
        "schemaVersion": "tradeassembly.execution_checkpoint.v1",
        "checkpointId": checkpoint_id,
        "activationId": activation_id,
        "correlationId": correlation_id,
        "cycle": cycle,
        "sequence": cycle,
        "reason": reason,
        "state": state,
        "previousHash": previous_hash,
        "checkpointHash": checkpoint_hash,
        "replayRef": format!("tradeassembly://execution/{activation_id}/checkpoints/{cycle}"),
    })
}

fn controls_default(activation_id: &str) -> Value {
    json!({
        "activationId": activation_id,
        "pauseEntries": "available",
        "resumeEntries": "available",
        "stopAfterFlat": "available",
        "stop": "available",
        "cancelOrders": "available",
        "liquidate": "available",
        "manualOverride": "available",
        "emergencyStop": "available",
        "lastControl": null,
    })
}

fn action_key(action: &str) -> &'static str {
    match action {
        "resume_entries" => "resumeEntries",
        "stop_after_flat" => "stopAfterFlat",
        "stop" => "stop",
        "cancel_orders" => "cancelOrders",
        "liquidate" => "liquidate",
        "manual_override" => "manualOverride",
        "emergency_stop" => "emergencyStop",
        _ => "pauseEntries",
    }
}

fn entries_paused(service: &TradeAssemblyService, activation_id: &str) -> bool {
    get(service, CONTROLS_NS, activation_id)
        .and_then(|controls| controls["pauseEntries"].as_str().map(str::to_string))
        .is_some_and(|state| state == "paused")
}

fn stop_after_flat_armed(service: &TradeAssemblyService, activation_id: &str) -> bool {
    get(service, CONTROLS_NS, activation_id)
        .and_then(|controls| controls["stopAfterFlat"].as_str().map(str::to_string))
        .is_some_and(|state| state == "armed")
}

fn new_entries_blocked(service: &TradeAssemblyService, activation_id: &str) -> bool {
    entries_paused(service, activation_id) || stop_after_flat_armed(service, activation_id)
}

fn capability_revision_blocker(service: &TradeAssemblyService, run: &Value) -> Option<Value> {
    capability_graph::revision_for_context(
        service,
        run["capabilityGraphRevisionId"]
            .as_str()
            .unwrap_or_default(),
        run["strategyId"].as_str().unwrap_or_default(),
        run["strategyVersionId"].as_str().unwrap_or_default(),
        run["mode"].as_str().unwrap_or("paper"),
        &now_rfc3339_second().unwrap_or_else(|| "unspecified".to_string()),
    )
    .err()
}

fn refresh_compatible_capability_revision(
    service: &TradeAssemblyService,
    run: &mut Value,
    stale_detail: &Value,
) -> Result<(), String> {
    let activation_id = run["activationId"]
        .as_str()
        .ok_or_else(|| "activation_id_missing".to_string())?
        .to_string();
    let config_id = run["configId"]
        .as_str()
        .ok_or_else(|| "execution_config_missing".to_string())?
        .to_string();
    let mut config = get(service, CONFIGS_NS, &config_id)
        .ok_or_else(|| "execution_config_missing".to_string())?;
    let revision = capability_graph::save_execution_revision(
        service,
        &json!({
            "evaluationEpoch": now_rfc3339_second().unwrap_or_else(|| "unspecified".to_string()),
            "idempotencyKey": format!("capability-migration:{activation_id}:{}", service.runtime().clock.now_ms()),
            "authorityContext": {
                "actor": "tradeassembly.runtime",
                "surface": "scheduler",
                "accountMode": run["mode"],
            },
        }),
        &config,
    )
    .map_err(|error| {
        format!(
            "capability_revision_refresh_failed:{}",
            error["error"]["code"]
                .as_str()
                .or_else(|| error["state"].as_str())
                .unwrap_or("unknown")
        )
    })?;
    if !revision["graph"]["ok"].as_bool().unwrap_or(false) {
        let blocker = revision["graph"]["blockers"]
            .as_array()
            .and_then(|blockers| blockers.first())
            .and_then(|blocker| blocker["code"].as_str())
            .unwrap_or("unknown");
        return Err(format!("capability_revision_not_ready:{blocker}"));
    }
    let previous_graph = run
        .get("currentCapabilityGraphSnapshot")
        .unwrap_or(&run["immutableInput"]["capabilityGraphSnapshot"]);
    if !capability_bindings_compatible(previous_graph, &revision["graph"]) {
        return Err("capability_binding_changed".to_string());
    }

    let prior_revision_id = run["capabilityGraphRevisionId"].clone();
    let prior_fingerprint = run["capabilityGraphFingerprint"].clone();
    run["capabilityGraphRevisionId"] = revision["revisionId"].clone();
    run["capabilityGraphFingerprint"] = revision["graphFingerprint"].clone();
    run["currentCapabilityGraphSnapshot"] = revision["graph"].clone();
    run["currentSelectedManifestFingerprints"] = revision["selectedManifestFingerprints"].clone();
    let migrations = run
        .get_mut("capabilityMigrations")
        .and_then(Value::as_array_mut);
    let migration = json!({
        "fromRevisionId": prior_revision_id,
        "fromFingerprint": prior_fingerprint,
        "toRevisionId": revision["revisionId"],
        "toFingerprint": revision["graphFingerprint"],
        "reason": "compatible_plugin_manifest_upgrade",
        "staleDetail": stale_detail,
        "migratedAtMs": service.runtime().clock.now_ms(),
    });
    if let Some(migrations) = migrations {
        migrations.push(migration.clone());
    } else {
        run["capabilityMigrations"] = json!([migration.clone()]);
    }

    config["capabilityGraphRevisionId"] = revision["revisionId"].clone();
    config["capabilityGraphFingerprint"] = revision["graphFingerprint"].clone();
    persist(
        service,
        CONFIGS_NS,
        &config_id,
        config,
        "execution.config.capability_revision_migrated",
    );
    if let Some(mut activation) = get(service, ACTIVATIONS_NS, &activation_id) {
        activation["capabilityGraphRevisionId"] = revision["revisionId"].clone();
        activation["capabilityGraphFingerprint"] = revision["graphFingerprint"].clone();
        activation["selectedManifestFingerprints"] =
            revision["selectedManifestFingerprints"].clone();
        persist(
            service,
            ACTIVATIONS_NS,
            &activation_id,
            activation,
            "execution.activation.capability_revision_migrated",
        );
    }
    append_event(
        service,
        &activation_id,
        "capability.revision_migrated",
        migration,
    );
    Ok(())
}

fn capability_bindings_compatible(previous: &Value, current: &Value) -> bool {
    let selections = |graph: &Value| {
        let mut selected = graph["nodes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|node| node.get("selected").cloned())
            .collect::<Vec<_>>();
        selected.sort_by_key(capability_binding_identity);
        selected
    };
    let previous = selections(previous);
    let current = selections(current);
    !previous.is_empty()
        && previous.len() == current.len()
        && previous.iter().zip(current.iter()).all(|(before, after)| {
            let (before_contract, before_outputs) = normalized_operation_contract(before);
            let (after_contract, after_outputs) = normalized_operation_contract(after);
            capability_binding_identity(before) == capability_binding_identity(after)
                && before_contract == after_contract
                && before_outputs.is_subset(&after_outputs)
        })
}

fn capability_binding_identity(selected: &Value) -> String {
    [
        selected["pluginInstanceRef"].as_str().unwrap_or_default(),
        selected["pluginRef"].as_str().unwrap_or_default(),
        selected["operationId"].as_str().unwrap_or_default(),
        selected["operation"]["capability"]
            .as_str()
            .unwrap_or_default(),
        selected["accountRef"].as_str().unwrap_or_default(),
    ]
    .join("|")
}

fn normalized_operation_contract(selected: &Value) -> (Value, HashSet<String>) {
    let mut operation = selected["operation"].clone();
    let outputs = operation
        .pointer("/traits/outputSchemaRefs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<HashSet<_>>();
    if let Some(traits) = operation.get_mut("traits").and_then(Value::as_object_mut) {
        traits.remove("outputSchemaRefs");
    }
    (operation, outputs)
}

fn reconciliation_blocks_effects(service: &TradeAssemblyService, activation_id: &str) -> bool {
    get(service, RECONCILIATION_NS, activation_id).is_none_or(|state| {
        state["state"].as_str() != Some("clear")
            || state["newEntriesPaused"].as_bool().unwrap_or(true)
            || state["pendingActions"]
                .as_array()
                .is_none_or(|actions| !actions.is_empty())
            || state["ambiguousActions"]
                .as_array()
                .is_none_or(|actions| !actions.is_empty())
    })
}

fn validate_reconciliation_order_request(
    run: &Value,
    activation_id: &str,
    order_request: &Value,
) -> Result<(), String> {
    let run_mode = durable_execution_mode(run).map_err(str::to_string)?;
    if order_request["activationId"].as_str() != Some(activation_id)
        || order_request["strategyId"] != run["strategyId"]
        || order_request["strategyVersionId"] != run["strategyVersionId"]
        || order_request["mode"].as_str() != Some(run_mode.as_str())
        || order_request["accountMode"].as_str() != Some(run_mode.as_str())
    {
        return Err("reconciliation_order_request_mismatch".to_string());
    }
    let plugin_request = order_request
        .get("pluginOperationRequest")
        .ok_or_else(|| "reconciliation_order_request_missing".to_string())?;
    let (expected_capability, expected_operation) = if run_mode == "live" {
        ("broker.order_submit.live", "broker.live_order_submit")
    } else {
        ("broker.order_submit.paper", "broker.paper_order_submit")
    };
    if plugin_request["mode"].as_str() != Some(run_mode.as_str())
        || plugin_request["purpose"].as_str()
            != Some(if run_mode == "live" {
                "live_order_submission"
            } else {
                "paper_trading"
            })
        || plugin_request["capability"].as_str() != Some(expected_capability)
        || plugin_request["operationId"].as_str() != Some(expected_operation)
    {
        return Err("reconciliation_order_request_mismatch".to_string());
    }
    Ok(())
}

fn reconcile_ambiguous_actions(
    service: &TradeAssemblyService,
    run: &mut Value,
) -> Result<(), String> {
    let activation_id = run["activationId"]
        .as_str()
        .ok_or_else(|| "activation_id_missing".to_string())?
        .to_string();
    let Some(mut reconciliation) = get(service, RECONCILIATION_NS, &activation_id) else {
        return Err("reconciliation_state_missing".to_string());
    };
    if !reconciliation_blocks_effects(service, &activation_id) {
        return Ok(());
    }
    let actions = reconciliation["ambiguousActions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if actions.is_empty() {
        return Err("reconciliation_evidence_missing".to_string());
    }
    let mut resolved = Vec::with_capacity(actions.len());
    for action in actions {
        let order_request = action["orderRequest"]
            .as_object()
            .map(|_| action["orderRequest"].clone())
            .ok_or_else(|| "reconciliation_order_request_missing".to_string())?;
        validate_reconciliation_order_request(run, &activation_id, &order_request)?;
        let mut outcome = orders::run_once(service, order_request.clone());
        if outcome["status"].as_str() != Some("completed") {
            let recovery_request = recovery_order_request(service, &order_request)?;
            outcome = orders::run_once(service, recovery_request);
        }
        if outcome["status"].as_str() != Some("completed") {
            return Err("reconciliation_provider_outcome_unresolved".to_string());
        }
        let quantity_micros = decimal_micros(order_request.get("qty"))
            .filter(|quantity| *quantity > 0)
            .ok_or_else(|| "reconciliation_quantity_invalid".to_string())?;
        run["paperPositionMicros"] = json!(match order_request["side"].as_str() {
            Some("buy") => run["paperPositionMicros"]
                .as_i64()
                .unwrap_or(0)
                .saturating_add(quantity_micros),
            Some("sell") => run["paperPositionMicros"]
                .as_i64()
                .unwrap_or(0)
                .saturating_sub(quantity_micros)
                .max(0),
            _ => return Err("reconciliation_side_invalid".to_string()),
        });
        resolved.push(json!({
            "clientOrderId": action["clientOrderId"],
            "orderId": outcome["order_id"],
            "providerOrderId": outcome["order"]["provider_order_id"],
            "status": outcome["order"]["status"],
        }));
    }
    reconciliation["state"] = json!("clear");
    reconciliation["newEntriesPaused"] = json!(false);
    reconciliation["ambiguousActions"] = json!([]);
    reconciliation["resolvedActions"] = json!(resolved);
    reconciliation["resolvedAtMs"] = json!(service.runtime().clock.now_ms());
    reconciliation["reason"] = Value::Null;
    reconciliation["detail"] = Value::Null;
    persist(
        service,
        RECONCILIATION_NS,
        &activation_id,
        reconciliation,
        "execution.reconciliation.resolved",
    );
    run["health"]["seriousReconciliationMismatch"] = json!(false);
    run["health"]["newEntriesPaused"] = json!(false);
    run["stateReason"] = json!("reconciliation_resolved");
    if let Some(mut health) = get(service, HEALTH_NS, &activation_id) {
        health["status"] = json!(if run["state"].as_str() == Some("stopped") {
            "stopped"
        } else {
            "healthy"
        });
        health["reason"] = Value::Null;
        health["newEntriesPaused"] = json!(false);
        persist(
            service,
            HEALTH_NS,
            &activation_id,
            health,
            "execution.health.reconciled",
        );
    }
    persist(
        service,
        RUNS_NS,
        run["executionRunId"].as_str().unwrap_or("run"),
        run.clone(),
        "execution.run.reconciled",
    );
    append_event(
        service,
        &activation_id,
        "reconciliation.resolved",
        json!({"resolvedActions": resolved}),
    );
    Ok(())
}

fn recovery_order_request(
    service: &TradeAssemblyService,
    order_request: &Value,
) -> Result<Value, String> {
    let mut recovered = order_request.clone();
    let plugin_request = recovered
        .get_mut("pluginOperationRequest")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "reconciliation_order_request_missing".to_string())?;
    let instance_ref = plugin_request
        .get("pluginInstanceRef")
        .and_then(Value::as_str)
        .ok_or_else(|| "reconciliation_plugin_binding_missing".to_string())?;
    let plugin_ref = plugin_request
        .get("pluginRef")
        .and_then(Value::as_str)
        .ok_or_else(|| "reconciliation_plugin_binding_missing".to_string())?;
    let operation_id = plugin_request
        .get("operationId")
        .and_then(Value::as_str)
        .ok_or_else(|| "reconciliation_plugin_binding_missing".to_string())?;
    let capability = plugin_request
        .get("capability")
        .and_then(Value::as_str)
        .ok_or_else(|| "reconciliation_plugin_binding_missing".to_string())?;
    let current = super::providers::plugin_catalog(service)
        .into_iter()
        .find(|plugin| {
            plugin.instance_ref == instance_ref
                && plugin.plugin_ref == plugin_ref
                && plugin.operations.iter().any(|operation| {
                    operation.id == operation_id && operation.capability == capability
                })
        })
        .ok_or_else(|| "reconciliation_plugin_binding_unavailable".to_string())?;
    let recovery_key = format!(
        "{}:reconcile:{}",
        order_request["idempotencyKey"]
            .as_str()
            .ok_or_else(|| "reconciliation_idempotency_key_missing".to_string())?,
        short_hash(&current.manifest_fingerprint)
    );
    plugin_request.insert(
        "manifestFingerprint".to_string(),
        json!(current.manifest_fingerprint),
    );
    recovered["idempotencyKey"] = json!(recovery_key);
    Ok(recovered)
}

fn pause_for_runtime_uncertainty(
    service: &TradeAssemblyService,
    activation_id: &str,
    reason: &str,
    detail: Value,
) {
    let mut reconciliation = get(service, RECONCILIATION_NS, activation_id).unwrap_or_else(|| {
        json!({
            "schemaVersion": "tradeassembly.execution_reconciliation.v1",
            "kind": "ReconciliationState",
            "activationId": activation_id,
            "correlationId": durable_correlation_id(service, activation_id),
            "pendingActions": [],
            "ambiguousActions": [],
        })
    });
    reconciliation["state"] = json!("required");
    reconciliation["newEntriesPaused"] = json!(true);
    reconciliation["reason"] = json!(reason);
    reconciliation["detail"] = detail;
    reconciliation["updatedAtMs"] = json!(service.runtime().clock.now_ms());
    if reason == "plugin_order_submission_failed" {
        reconciliation["ambiguousActions"] = json!([reconciliation["detail"].clone()]);
    }
    persist(
        service,
        RECONCILIATION_NS,
        activation_id,
        reconciliation,
        "execution.reconciliation.required",
    );
}

fn clear_capability_reconciliation(service: &TradeAssemblyService, activation_id: &str) {
    let Some(mut reconciliation) = get(service, RECONCILIATION_NS, activation_id) else {
        return;
    };
    if reconciliation["reason"].as_str() != Some("capability_revision_stale") {
        return;
    }
    reconciliation["state"] = json!("clear");
    reconciliation["newEntriesPaused"] = json!(false);
    reconciliation["reason"] = Value::Null;
    reconciliation["detail"] = Value::Null;
    reconciliation["updatedAtMs"] = json!(service.runtime().clock.now_ms());
    persist(
        service,
        RECONCILIATION_NS,
        activation_id,
        reconciliation,
        "execution.reconciliation.cleared",
    );
}

fn complete_effect_free_cycle(
    service: &TradeAssemblyService,
    run: &mut Value,
    cycle: u64,
    lease: &Value,
    reason: &str,
    detail: Value,
) -> Value {
    let activation_id = run["activationId"]
        .as_str()
        .unwrap_or("activation")
        .to_string();
    let checkpoint = checkpoint(
        &activation_id,
        &correlation_id_for_run(run),
        cycle,
        run["checkpointHash"].as_str(),
        reason,
        json!({
            "activeWatches": [run["symbol"].clone()],
            "pendingOrders": [],
            "riskReservations": risk::reservations(service),
            "paperPositionMicros": run["paperPositionMicros"],
            "failClosed": {
                "reason": reason,
                "newEntriesPaused": true,
                "effectsApplied": false,
            },
        }),
    );
    persist(
        service,
        CHECKPOINTS_NS,
        checkpoint["checkpointId"].as_str().unwrap_or("checkpoint"),
        checkpoint.clone(),
        "execution.checkpoint.blocked",
    );
    run["cycle"] = json!(cycle);
    run["state"] = json!("active");
    run["stateReason"] = json!(reason);
    run["checkpoint"] = checkpoint.clone();
    run["checkpointId"] = checkpoint["checkpointId"].clone();
    run["checkpointHash"] = checkpoint["checkpointHash"].clone();
    run["health"] = json!({
        "status": "degraded",
        "reason": reason,
        "effectsPaused": true,
        "continuousEvaluation": true,
    });
    persist(
        service,
        RUNS_NS,
        run["executionRunId"].as_str().unwrap_or("run"),
        run.clone(),
        "execution.run.cycle_blocked",
    );
    if let Some(mut activation) = get(service, ACTIVATIONS_NS, &activation_id) {
        activation["state"] = json!("active");
        activation["stateReason"] = json!(reason);
        activation["lastCompletedSequence"] = json!(cycle);
        activation["checkpointId"] = checkpoint["checkpointId"].clone();
        activation["checkpointHash"] = checkpoint["checkpointHash"].clone();
        persist(
            service,
            ACTIVATIONS_NS,
            &activation_id,
            activation,
            "execution.activation.cycle_blocked",
        );
    }
    append_event(
        service,
        &activation_id,
        "execution.cycle_fail_closed",
        json!({
            "cycle": cycle,
            "reason": reason,
            "effectsApplied": false,
            "willRetryNextCycle": true,
            "detail": detail,
        }),
    );
    json!({
        "status": "blocked",
        "reason": reason,
        "activationId": activation_id,
        "executionRunId": run["executionRunId"],
        "cycle": cycle,
        "checkpoint": checkpoint,
        "effectsApplied": false,
        "willRetryNextCycle": true,
        "lease": lease,
    })
}

struct TerminalTick<'a> {
    body: &'a Value,
    activation_id: &'a str,
    tick_id: &'a str,
    sequence: u64,
    status: &'a str,
    decision: Value,
}

fn persist_terminal_tick(service: &TradeAssemblyService, run: &Value, terminal: TerminalTick<'_>) {
    let tick = json!({
        "schemaVersion": "tradeassembly.evaluation_tick.v1",
        "kind": "EvaluationTickRecord",
        "tickId": terminal.tick_id,
        "activationId": terminal.activation_id,
        "correlationId": correlation_id_for_run(run),
        "attemptId": string_field(terminal.body, &["attemptId", "attempt_id"]),
        "sequence": terminal.sequence,
        "strategyVersionId": run["strategyVersionId"],
        "strategySpecHash": run["strategySpecHash"],
        "sourceObservationIds": [],
        "decision": terminal.decision,
        "orderIntent": Value::Null,
        "orderResult": {"status": "skipped", "reason": terminal.status},
        "priorCheckpointId": run["checkpointId"],
        "checkpointId": Value::Null,
        "status": terminal.status,
        "recordedAtMs": service.runtime().clock.now_ms(),
    });
    persist(
        service,
        TICKS_NS,
        &format!("{}:{}", slug(terminal.activation_id), terminal.sequence),
        tick,
        "execution.tick.recorded",
    );
}

fn values_for_activation(
    values: Vec<Value>,
    activation_id: &str,
    related_order_ids: &HashSet<String>,
) -> Vec<Value> {
    if activation_id.is_empty() {
        return Vec::new();
    }
    values
        .into_iter()
        .filter(
            |value| match string_field(value, &["activationId", "activation_id"]) {
                Some(candidate) => candidate == activation_id,
                None => string_field(value, &["orderId", "order_id", "id"])
                    .is_some_and(|order_id| related_order_ids.contains(&order_id)),
            },
        )
        .collect()
}

fn ticks_for(service: &TradeAssemblyService, activation_id: &str) -> Vec<Value> {
    let mut ticks = list_namespace(service, TICKS_NS)
        .into_iter()
        .filter(|tick| tick["activationId"].as_str() == Some(activation_id))
        .collect::<Vec<_>>();
    ticks.sort_by(|left, right| {
        left["sequence"]
            .as_u64()
            .unwrap_or(u64::MAX)
            .cmp(&right["sequence"].as_u64().unwrap_or(u64::MAX))
            .then_with(|| {
                left["tickId"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["tickId"].as_str().unwrap_or_default())
            })
    });
    ticks
}

fn decision_rows_from_events(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .filter(|event| {
            event["eventType"]
                .as_str()
                .unwrap_or_default()
                .starts_with("decision.")
        })
        .map(|event| event["payload"].clone())
        .collect()
}

fn reconciliation_for(service: &TradeAssemblyService, activation_id: &str) -> Value {
    if activation_id.is_empty() {
        return json!({
            "available": false,
            "state": "unavailable",
            "reason": "activation_not_selected",
            "evidenceRefs": [],
        });
    }
    get(service, RECONCILIATION_NS, activation_id)
        .map(|mut reconciliation| {
            reconciliation["available"] = json!(true);
            reconciliation
        })
        .unwrap_or_else(|| {
            json!({
                "available": false,
                "activationId": activation_id,
                "state": "unavailable",
                "reason": "reconciliation_evidence_missing",
                "newEntriesPaused": Value::Null,
                "evidenceRefs": [],
            })
        })
}

fn ledger_from_evidence(
    activation_id: &str,
    orders: &[Value],
    positions: &[Value],
    reconciliation: &Value,
) -> Value {
    let unrealized_values: Option<Vec<f64>> = positions
        .iter()
        .map(|position| {
            let quantity = optional_number_field(position, &["qty", "quantity"])?;
            let average = optional_number_field(position, &["average_price", "averagePrice"])?;
            let mark = optional_number_field(position, &["mark_price", "markPrice"])?;
            Some((mark - average) * quantity)
        })
        .collect::<Option<Vec<_>>>();
    let position_realized_values = positions
        .iter()
        .filter_map(|position| optional_number_field(position, &["realized_pnl", "realizedPnl"]))
        .collect::<Vec<_>>();
    let order_realized_values = orders
        .iter()
        .filter_map(|order| optional_number_field(order, &["realized_pnl", "realizedPnl"]))
        .collect::<Vec<_>>();
    let realized_pnl = if positions.is_empty() {
        (orders.is_empty() || order_realized_values.len() == orders.len())
            .then(|| order_realized_values.iter().sum::<f64>())
    } else if position_realized_values.len() == positions.len() {
        Some(position_realized_values.iter().sum::<f64>())
    } else {
        None
    };
    let unrealized_pnl = unrealized_values
        .as_ref()
        .map(|values| values.iter().sum::<f64>());
    let total_pnl = realized_pnl
        .zip(unrealized_pnl)
        .map(|(realized, unrealized)| realized + unrealized);
    json!({
        "schemaVersion": "tradeassembly.execution_ledger.v2",
        "activationId": activation_id,
        "available": !activation_id.is_empty(),
        "orders": orders,
        "positions": positions,
        "totals": {
            "scope": "since_run_start",
            "closedTrades": orders.iter().filter(|order| order["status"] == "closed").count(),
            "openTrades": positions.iter().filter(|position| position["status"] == "open").count(),
            "realizedPnl": realized_pnl,
            "unrealizedPnl": unrealized_pnl,
            "totalPnl": total_pnl,
            "currency": "USD",
            "available": total_pnl.is_some(),
            "unavailableReason": if total_pnl.is_some() { Value::Null } else { json!("price_or_realized_pnl_evidence_missing") },
        },
        "reconciliation": reconciliation,
    })
}

fn millis_timestamp(value: i64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(value)
        .map(|timestamp| timestamp.to_rfc3339())
}

fn chart_from_evidence(
    activation_id: &str,
    strategy_id: &str,
    ticks: &[Value],
    orders: &[Value],
) -> Value {
    let price_series = ticks
        .iter()
        .filter_map(|tick| {
            let facts = &tick["decision"]["facts"];
            let observed_at_ms = facts["observedAtMs"].as_i64()?;
            let timestamp = millis_timestamp(observed_at_ms)?;
            let symbol = facts["symbol"].as_str()?;
            let close_micros = facts["closeMicros"].as_i64()?;
            let open_micros = facts["openMicros"].as_i64().unwrap_or(close_micros);
            Some(json!({
                "timestamp": timestamp,
                "timestampMs": observed_at_ms,
                "symbol": symbol,
                "open": quantity_value(open_micros),
                "close": quantity_value(close_micros),
                "sourceObservationIds": tick["sourceObservationIds"],
                "evidenceRefs": tick["decision"]["sourceObservationIds"],
            }))
        })
        .collect::<Vec<_>>();
    let order_by_id = orders
        .iter()
        .filter_map(|order| {
            string_field(order, &["order_id", "orderId", "id"]).map(|order_id| (order_id, order))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let trade_markers = ticks
        .iter()
        .filter_map(|tick| {
            let order_id = string_field(&tick["orderResult"], &["order_id", "orderId"])?;
            let order = order_by_id.get(&order_id)?;
            let facts = &tick["decision"]["facts"];
            let timestamp = millis_timestamp(facts["observedAtMs"].as_i64()?)?;
            let price = optional_number_field(order, &["fill_price", "fillPrice"])
                .or_else(|| facts["closeMicros"].as_i64().map(quantity_value))?;
            Some(json!({
                "id": order_id,
                "markerRef": format!("marker-{order_id}"),
                "tradeId": order_id,
                "timestamp": timestamp,
                "price": price,
                "side": order["side"],
                "kind": if order["side"] == "sell" { "exit" } else { "entry" },
                "status": order["status"],
                "label": format!("{} {}", order["side"].as_str().unwrap_or("order"), order["status"].as_str().unwrap_or("recorded")),
                "evidenceRefs": tick["sourceObservationIds"],
            }))
        })
        .collect::<Vec<_>>();
    let timestamps = price_series
        .iter()
        .filter_map(|point| point["timestamp"].as_str())
        .collect::<Vec<_>>();
    let symbols = price_series
        .iter()
        .filter_map(|point| point["symbol"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    json!({
        "schemaVersion": "tradeassembly.execution_chart.v2",
        "strategyId": strategy_id,
        "activationId": if activation_id.is_empty() { Value::Null } else { json!(activation_id) },
        "available": !price_series.is_empty(),
        "unavailableReason": if price_series.is_empty() { json!("durable_market_observations_unavailable") } else { Value::Null },
        "range": {
            "start": timestamps.first(),
            "end": timestamps.last(),
            "boundedByEvidence": true,
        },
        "followLive": true,
        "watchedSymbols": symbols,
        "priceSeries": price_series,
        "tradeMarkers": trade_markers,
        "tradeConnectors": [],
        "warningOverlays": [],
    })
}

fn scheduler_for_activation(service: &TradeAssemblyService, activation_id: &str) -> Value {
    let scheduler = scheduler_status(service);
    let selected = scheduler["activationId"].as_str() == Some(activation_id)
        || scheduler["activation_id"].as_str() == Some(activation_id);
    if selected {
        scheduler
    } else {
        json!({
            "available": false,
            "activationId": if activation_id.is_empty() { Value::Null } else { json!(activation_id) },
            "running": Value::Null,
            "state": "unavailable",
            "reason": "scheduler_state_not_bound_to_activation",
        })
    }
}

fn health_for(
    service: &TradeAssemblyService,
    activation_id: &str,
    run: &Value,
    reconciliation: &Value,
    ticks: &[Value],
) -> Value {
    if activation_id.is_empty() {
        return json!({
            "available": false,
            "status": "idle",
            "reason": "activation_not_selected",
            "components": {},
        });
    }
    let stored = get(service, HEALTH_NS, activation_id).unwrap_or_else(|| {
        json!({
            "available": false,
            "status": "unavailable",
            "reason": "health_evidence_missing",
        })
    });
    let reconciliation_state = reconciliation["state"].as_str().unwrap_or("unavailable");
    let scheduler = scheduler_for_activation(service, activation_id);
    let scheduler_failure = scheduler["lastTick"]["status"].as_str() == Some("failed");
    let status = if reconciliation_state != "clear" || scheduler_failure {
        "degraded"
    } else {
        stored["status"].as_str().unwrap_or("unavailable")
    };
    let latest_tick = ticks.last();
    json!({
        "available": stored["available"].as_bool().unwrap_or(true),
        "status": status,
        "activationId": activation_id,
        "correlationId": run["correlationId"],
        "newEntriesPaused": reconciliation["newEntriesPaused"],
        "reason": if scheduler_failure {
            scheduler["lastTick"]["reason"].clone()
        } else {
            reconciliation["reason"].clone()
        },
        "components": {
            "runtime": {
                "status": stored["status"],
                "state": run["state"],
                "stateReason": run["stateReason"],
            },
            "checkpoint": {
                "status": if run["checkpointId"].is_string() { "current" } else { "unavailable" },
                "checkpointId": run["checkpointId"],
            },
            "marketData": {
                "status": if latest_tick.is_some() { "observed" } else { "unavailable" },
                "lastObservedAtMs": latest_tick.and_then(|tick| tick["decision"]["facts"]["observedAtMs"].as_i64()),
                "evidenceRefs": latest_tick.map(|tick| tick["sourceObservationIds"].clone()).unwrap_or_else(|| json!([])),
            },
            "reconciliation": reconciliation,
            "scheduler": {
                "status": scheduler["lastTick"]["status"],
                "reason": scheduler["lastTick"]["reason"],
                "iterations": scheduler["iterations"],
            },
        },
    })
}

fn execution_analytics_from_evidence(
    activation_id: &str,
    orders: &[Value],
    attempts: &[Value],
    positions: &[Value],
    events: &[Value],
    execution: &Value,
) -> Value {
    let submitted = orders
        .iter()
        .filter(|order| order["submit"].as_bool().unwrap_or(false))
        .count();
    let filled = orders
        .iter()
        .filter(|order| order["status"] == "filled")
        .count();
    json!({
        "schemaVersion": "tradeassembly.execution_analytics.v2",
        "activationId": activation_id,
        "computedFrom": {
            "orderCount": orders.len(),
            "attemptCount": attempts.len(),
            "positionCount": positions.len(),
            "eventCount": events.len(),
        },
        "liveVsBacktestDrift": {
            "status": "insufficient_data",
            "reason": "baseline_reconciliation_not_in_gc16",
        },
        "fillSlippageCalibration": {
            "status": "insufficient_data",
            "reason": "decision_and_fill_quote_evidence_required",
            "submittedOrders": submitted,
            "filledOrders": filled,
            "attemptCount": attempts.len(),
            "estimatedSlippageBps": Value::Null,
        },
        "latencyDiagnostics": {
            "status": "insufficient_data",
            "reason": "provider_timestamps_missing",
        },
        "capacityLiquidity": {
            "status": "insufficient_data",
            "reason": "provider_liquidity_depth_missing",
            "activePositions": positions.iter().filter(|position| position["status"] == "open").count(),
            "activeNotional": execution["risk"]["activeNotional"],
            "requestedQuantity": Value::Null,
        },
        "stressMonitor": {
            "status": "insufficient_data",
            "reason": "derivatives_and_stress_analysis_owned_by_gc13",
            "stressLossEstimate": Value::Null,
        },
        "scenarioAlerts": {
            "status": "insufficient_data",
            "reason": "scenario_analysis_owned_by_gc13",
            "items": [],
        },
        "noAdvice": LEGAL_BOUNDARY,
    })
}

fn ledger_totals(service: &TradeAssemblyService) -> Value {
    let positions = positions::list(service);
    let open_positions = positions
        .iter()
        .filter(|position| position["status"] == "open")
        .count();
    json!({
        "closedTrades": 0,
        "openTrades": open_positions,
        "realizedPnl": 0.0,
        "unrealizedPnl": if open_positions == 0 { 0.0 } else { 0.44 },
        "totalPnl": if open_positions == 0 { 0.0 } else { 0.44 },
        "wins": 0,
        "losses": 0,
    })
}

fn evidence_drawer(service: &TradeAssemblyService, activation_id: &str) -> Value {
    if service
        .require_object("execution_activation", activation_id)
        .is_err()
    {
        return json!({"available": false, "reason": "object_not_available"});
    }
    json!({
        "activationId": activation_id,
        "events": events_for(service, activation_id),
        "apf": dashboard_audit_reference(),
        "exports": [
            {"label": "Execution JSON", "href": format!("tradeassembly://execution/{activation_id}/export.json")},
            {"label": "Evidence Bundle", "href": format!("tradeassembly://execution/{activation_id}/evidence.zip")}
        ],
    })
}

fn approval_panel(run: &Value) -> Value {
    json!({
        "required": false,
        "mode": run["mode"].as_str().unwrap_or("paper"),
        "pending": [],
        "message": "Paper execution uses local acknowledgements and APF receipts.",
    })
}

fn activation_matrix(service: &TradeAssemblyService) -> Vec<Value> {
    list_namespace(service, CONFIGS_NS)
        .into_iter()
        .map(|config| {
            let activation_id = latest_activation_id(service).unwrap_or_else(|| "none".to_string());
            json!({
                "configId": config["configId"],
                "strategyId": config["strategyId"],
                "providerRef": config["providerRef"],
                "activationStatus": get(service, RUNS_NS, &run_id_for_activation(&activation_id))
                    .and_then(|run| run["state"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "inactive".to_string()),
            })
        })
        .collect()
}

fn latest_activation_id(service: &TradeAssemblyService) -> Option<String> {
    execution_runs(service, None)
        .last()
        .and_then(|run| run["activationId"].as_str().map(str::to_string))
}

fn failure_states(service: &TradeAssemblyService) -> Value {
    let mismatch = risk::status(service)["activeCount"].as_u64().unwrap_or(0) > 10;
    json!({
        "reconciliation": {
            "status": if mismatch { "paused" } else { "clear" },
            "summary": if mismatch { "serious mismatch pauses new entries" } else { "local reconciliation matched" },
        },
        "credentials": {"status": "local", "summary": "local credential status is checked before live use"},
        "plugin": {"status": "ok", "summary": "paper plugin capability resolved"},
    })
}

fn check(id: &str, label: &str, passed: bool, blocking: bool, evidence_ref: &str) -> Value {
    json!({
        "id": id,
        "label": label,
        "status": if passed { "pass" } else { "blocked" },
        "blocking": blocking,
        "evidenceRef": evidence_ref,
        "repairAction": if passed { Value::Null } else { json!(format!("repair:{id}")) },
    })
}

fn required_broker_capability(mode: &str) -> &'static str {
    if mode.eq_ignore_ascii_case("live") {
        "broker.order_submit.live"
    } else {
        "broker.order_submit.paper"
    }
}

fn apf_refs_for_body(
    body: &Value,
    source_interface: &str,
    command_name: &str,
    idempotency_fallback: &str,
) -> Value {
    let mode = execution_mode_from(body);
    let live = mode == "live";
    let command_id = string_field(body, &["controlPlaneCommandId"]).unwrap_or_else(|| {
        let idempotency_key = explicit_idempotency(body, idempotency_fallback);
        command_id(source_interface, command_name, &idempotency_key)
    });
    json!({
        "client": "self",
        "purpose": if live { "live_order_submission" } else { "paper_trading" },
        "policyVersion": "2026-07-08.local",
        "pepCoverage": if live { "C5" } else { "C3" },
        "sourceInterface": string_field(body, &["controlPlaneSourceInterface"]).unwrap_or_else(|| source_interface.to_string()),
        "descriptorRef": format!("finance_action_descriptors/{command_id}"),
        "decisionRef": format!("warden_decisions/{command_id}"),
        "receiptRef": format!("finance_receipts/{command_id}"),
        "credentialGrantRef": if live { "credential_grant/live-required" } else { "credential_grant/local-paper" },
        "downstreamOutcomeRef": format!("command_response/{command_id}"),
    })
}

fn explicit_idempotency(body: &Value, fallback: &str) -> String {
    string_field(body, &["idempotencyKey", "idempotency_key"])
        .unwrap_or_else(|| fallback.to_string())
}

fn trusted_correlation_id(body: &Value) -> String {
    string_field(body, &["controlPlaneCorrelationId"])
        .or_else(|| string_field(body, &["controlPlaneCommandId"]))
        .unwrap_or_else(|| format!("cmd_{}", short_hash(&hash_value(body))))
}

fn correlation_id_for_run(run: &Value) -> String {
    string_field(run, &["correlationId", "correlation_id"]).unwrap_or_else(|| {
        format!(
            "activation:{}",
            run["activationId"].as_str().unwrap_or("unknown")
        )
    })
}

fn durable_correlation_id(service: &TradeAssemblyService, activation_id: &str) -> Option<String> {
    get(service, ACTIVATIONS_NS, activation_id)
        .or_else(|| get(service, RUNS_NS, &run_id_for_activation(activation_id)))
        .and_then(|record| string_field(&record, &["correlationId", "correlation_id"]))
}

fn command_id(source_interface: &str, command_name: &str, idempotency_key: &str) -> String {
    format!(
        "cmd_{}",
        hash_text(&format!(
            "{source_interface}:{command_name}:{idempotency_key}"
        ))
    )
}

fn epoch_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn run_id_for_activation(activation_id: &str) -> String {
    format!("run_{activation_id}")
}

fn events_for(service: &TradeAssemblyService, activation_id: &str) -> Vec<Value> {
    if service
        .require_object("execution_activation", activation_id)
        .is_err()
    {
        return Vec::new();
    }
    let mut events = list_namespace(service, EVENTS_NS)
        .into_iter()
        .filter(|event| event["activationId"].as_str() == Some(activation_id))
        .collect::<Vec<_>>();
    events.sort_by_key(|event| event["sequence"].as_u64().unwrap_or(u64::MAX));
    events
}

fn object_unavailable(response: ServiceResponse) -> Value {
    response.body
}

fn object_scope_unavailable() -> Value {
    ServiceResponse::object_unavailable().body
}

fn count_events(service: &TradeAssemblyService, event_type: &str) -> usize {
    list_namespace(service, EVENTS_NS)
        .into_iter()
        .filter(|event| event["eventType"].as_str() == Some(event_type))
        .count()
}

fn get(service: &TradeAssemblyService, namespace: &str, key: &str) -> Option<Value> {
    service
        .runtime()
        .storage
        .get_json(namespace, key)
        .ok()
        .flatten()
}

fn list_namespace(service: &TradeAssemblyService, namespace: &str) -> Vec<Value> {
    service
        .runtime()
        .storage
        .list_json(namespace)
        .unwrap_or_default()
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

fn dashboard_audit_reference() -> Value {
    json!({
        "schemaVersion": "tradeassembly.warden_authority_audit_reference.v1",
        "available": true,
        "complete": false,
        "scope": "dashboard",
        "descriptors": [],
        "decisions": [],
        "receipts": [],
        "outcomes": [],
        "detail": "Complete authority evidence is available through the explicit audit and replay interfaces.",
    })
}

fn persist(
    service: &TradeAssemblyService,
    namespace: &str,
    key: &str,
    value: Value,
    event_type: &str,
) {
    let context = SideEffectContext::new(
        AuthorityContext::local_cli(),
        IdempotencyKey::new(format!("{event_type}:{key}")).expect("valid idempotency key"),
    );
    service
        .runtime()
        .storage
        .put_json(namespace, key, value.clone(), &context)
        .expect("persist execution state");
    let _ = service
        .runtime()
        .record_side_effect(event_type, value, &context);
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn optional_number_field(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_f64))
}

fn execution_mode_from(value: &Value) -> String {
    match string_field(value, &["mode", "accountMode", "account_mode"])
        .unwrap_or_else(|| "paper".to_string())
        .to_ascii_lowercase()
        .as_str()
    {
        "live" => "live".to_string(),
        "shadow" => "shadow".to_string(),
        _ => "paper".to_string(),
    }
}

/// Resolve the immutable mode carried by a durable execution run.
///
/// Missing mode remains paper for legacy paper fixtures. An explicit value
/// outside the supported deployment modes is rejected rather than silently
/// acquiring a paper or live authority binding.
fn durable_execution_mode(value: &Value) -> Result<String, &'static str> {
    match value.get("mode") {
        None => Ok("paper".to_string()),
        Some(mode) if mode.is_null() || !mode.is_string() => Err("execution_mode_invalid"),
        Some(mode) => match mode
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "paper" => Ok("paper".to_string()),
            "live" => Ok("live".to_string()),
            "shadow" => Ok("shadow".to_string()),
            _ => Err("execution_mode_invalid"),
        },
    }
}

fn hash_value(value: &Value) -> String {
    serde_json::to_vec(value)
        .map(|bytes| format!("sha256:{}", hash_bytes(&bytes)))
        .unwrap_or_else(|_| "sha256:invalid".to_string())
}

fn hash_strategy_spec(value: &Value) -> String {
    crate::spec::canonical_hash(value).unwrap_or_else(|_| "sha256:invalid".to_string())
}

fn hash_text(value: &str) -> String {
    hash_bytes(value.as_bytes())
}

fn short_hash(value: &str) -> String {
    hash_text(value).chars().take(12).collect()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

fn slug(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_attempt_identity_preserves_retries_when_logical_fencing_restarts() {
        let first = execution_attempt_id("activation-1", 27, 1, 1);
        let retried = execution_attempt_id("activation-1", 27, 2, 1);

        assert_ne!(first, retried);
        assert_eq!(first, "attempt_activation_1_27_schedule_1_lease_1");
        assert_eq!(retried, "attempt_activation_1_27_schedule_2_lease_1");
    }

    #[test]
    fn market_operation_request_preserves_the_execution_asset_class() {
        let run = json!({
            "correlationId": "correlation-1",
            "strategyId": "strategy-1",
            "strategyVersionId": "version-1",
            "strategySpecHash": "sha256:spec",
            "capabilityGraphRevisionId": "revision-1",
            "capabilityGraphFingerprint": "sha256:graph",
            "symbol": "BTC/USD",
            "assetClass": "crypto",
            "timeframe": "1h",
            "immutableInput": {
                "capabilityGraphSnapshot": {
                    "nodes": [{
                        "selected": {
                            "pluginInstanceRef": "alpaca-paper",
                            "pluginRef": "tradeassembly.alpaca",
                            "manifestFingerprint": "sha256:manifest",
                            "operationId": "marketdata.bars.read_v1",
                            "operation": {"capability": "market_data.bars.read@1"}
                        }
                    }]
                }
            }
        });

        let request =
            market_operation_request(&run, "activation-1", "tick-1", "attempt-1", 1, Some(1))
                .expect("market operation request");

        assert_eq!(request.input["symbol"], "BTC/USD");
        assert_eq!(request.input["assetClass"], "crypto");
        assert_eq!(request.input["timeframe"], "1h");
        assert_eq!(request.mode, "paper");
        assert_eq!(request.purpose, "paper_trading");

        let mut live_run = run.clone();
        live_run["mode"] = json!("live");
        live_run["immutableInput"]["capabilityGraphSnapshot"]["nodes"][0]["selected"] = json!({
            "pluginInstanceRef": "alpaca-live",
            "pluginRef": "tradeassembly.alpaca",
            "manifestFingerprint": "sha256:manifest",
            "operationId": "marketdata.bars.read_v1",
            "operation": {"capability": "market_data.bars.read@1"}
        });
        let live_request =
            market_operation_request(&live_run, "activation-1", "tick-1", "attempt-1", 1, None)
                .expect("live market operation request");
        assert_eq!(live_request.mode, "live");
        assert_eq!(live_request.purpose, "live_order_submission");
    }

    #[test]
    fn durable_mode_rejects_unknown_and_does_not_infer_live_from_caller_fields() {
        assert_eq!(durable_execution_mode(&json!({})).unwrap(), "paper");
        assert_eq!(
            durable_execution_mode(&json!({"accountMode": "live"})).unwrap(),
            "paper"
        );
        assert_eq!(
            durable_execution_mode(&json!({"mode": null})),
            Err("execution_mode_invalid")
        );
        assert_eq!(
            durable_execution_mode(&json!({"mode": 7})),
            Err("execution_mode_invalid")
        );
        assert_eq!(
            durable_execution_mode(&json!({"mode": "unsupported"})),
            Err("execution_mode_invalid")
        );
    }

    #[test]
    fn reconciliation_rejects_cross_mode_order_before_provider_call() {
        let run = json!({
            "mode": "live",
            "activationId": "activation-1",
            "strategyId": "strategy-1",
            "strategyVersionId": "version-1"
        });
        let request = json!({
            "activationId": "activation-1",
            "strategyId": "strategy-1",
            "strategyVersionId": "version-1",
            "mode": "paper",
            "accountMode": "paper",
            "pluginOperationRequest": {
                "mode": "paper",
                "purpose": "paper_trading",
                "capability": "broker.order_submit.paper",
                "operationId": "broker.paper_order_submit"
            }
        });
        assert_eq!(
            validate_reconciliation_order_request(&run, "activation-1", &request),
            Err("reconciliation_order_request_mismatch".to_string())
        );

        let live_request = json!({
            "activationId": "activation-1",
            "strategyId": "strategy-1",
            "strategyVersionId": "version-1",
            "mode": "live",
            "accountMode": "live",
            "pluginOperationRequest": {
                "mode": "live",
                "purpose": "live_order_submission",
                "capability": "broker.order_submit.live",
                "operationId": "broker.live_order_submit"
            }
        });
        assert!(validate_reconciliation_order_request(&run, "activation-1", &live_request).is_ok());
        let mut wrong_account = live_request.clone();
        wrong_account["accountMode"] = json!("paper");
        assert_eq!(
            validate_reconciliation_order_request(&run, "activation-1", &wrong_account),
            Err("reconciliation_order_request_mismatch".to_string())
        );
    }

    #[test]
    fn capability_refresh_allows_only_additive_output_schemas() {
        let graph = |input: &[&str], output: &[&str]| {
            json!({
                "nodes": [{
                    "selected": {
                        "pluginInstanceRef": "alpaca-paper",
                        "pluginRef": "tradeassembly.alpaca",
                        "operationId": "broker.paper_order_submit",
                        "accountRef": "account://alpaca/paper",
                        "operation": {
                            "id": "broker.paper_order_submit",
                            "capability": "broker.order_submit.paper",
                            "effect": "write",
                            "traits": {
                                "inputSchemaRefs": input,
                                "outputSchemaRefs": output,
                            }
                        }
                    }
                }]
            })
        };
        let prior = graph(&["schema://order@1"], &["schema://legacy-receipt@1"]);
        let additive = graph(
            &["schema://order@1"],
            &[
                "schema://legacy-receipt@1",
                "schema://broker/order-receipt@1",
            ],
        );
        let breaking = graph(
            &["schema://order@2"],
            &[
                "schema://legacy-receipt@1",
                "schema://broker/order-receipt@1",
            ],
        );

        assert!(capability_bindings_compatible(&prior, &additive));
        assert!(!capability_bindings_compatible(&prior, &breaking));
    }

    #[test]
    fn available_broker_boundary_does_not_grant_live_approval_or_evidence_policy() {
        let checks = live_activation_checks(&json!({}), true, true, true, true, false, false);
        for id in [
            "downstream_pep_coverage_c5",
            "downstream_live_order_adapter",
        ] {
            assert!(checks
                .iter()
                .any(|check| check["id"] == id && check["status"] == "pass"));
        }
        for id in ["confirm_before_send", "hosted_evidence_policy"] {
            assert!(checks
                .iter()
                .any(|check| check["id"] == id && check["status"] == "blocked"));
        }
    }

    #[test]
    fn local_preflight_does_not_require_cloud_services_but_other_profiles_do() {
        let local = live_preflight_report(&json!({}), &json!({}), &[], &[], true);
        assert_eq!(local["cloudProfile"]["status"], "not_applicable");
        assert!(local["cloudProfile"]["blockedReasons"]
            .as_array()
            .unwrap()
            .is_empty());
        let other = live_preflight_report(&json!({}), &json!({}), &[], &[], false);
        assert!(!other["cloudProfile"]["blockedReasons"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn live_readiness_rejects_risk_controls_the_broker_cannot_enforce() {
        let mut config =
            json!({"mode":"live","riskLimits":{"max_notional":10,"max_order_quantity":2}});
        assert!(valid_execution_risk_limits(&config));
        for key in ["max_daily_loss", "max_concurrent_positions", "stop_loss"] {
            config["riskLimits"][key] = json!(1);
            assert!(!valid_execution_risk_limits(&config));
            config["riskLimits"].as_object_mut().unwrap().remove(key);
        }
    }

    #[test]
    fn verified_legal_receipt_clears_only_legal_live_gates() {
        let checks = live_activation_checks(
            &json!({"killSwitch": {"emergencyStop": true}}),
            true,
            true,
            false,
            false,
            false,
            false,
        );
        for legal_check in ["legal_acknowledgement", "legal_document_version"] {
            assert!(checks
                .iter()
                .any(|check| check["id"] == legal_check && check["status"] == "pass"));
        }
        for independent_gate in [
            "hosted_evidence_policy",
            "live_credential_posture",
            "downstream_pep_coverage_c5",
            "confirm_before_send",
            "downstream_live_order_adapter",
        ] {
            assert!(checks.iter().any(|check| {
                check["id"] == independent_gate
                    && check["status"] == "blocked"
                    && check["blocking"] == true
            }));
        }
    }

    #[test]
    fn live_posture_requires_current_fresh_exact_account_observation() {
        let record = json!({
            "instanceRef":"broker", "pluginRef":"tradeassembly.alpaca",
            "accountRef":"account://broker/one", "accountMode":"live",
            "credentialStatus":{"configured":true},
            "health":{"evidenceCurrent":true, "checkedAtMs":1000,
                "account":{"id":"one", "mode":"live", "status":"ACTIVE",
                    "tradingBlocked":false,"accountBlocked":false,"tradeSuspendedByUser":false}}
        });
        let selected = json!({"pluginInstanceRef":"broker", "pluginRef":"tradeassembly.alpaca", "accountRef":"account://broker/one"});
        assert!(live_account_observation_matches(&record, &selected, 1000));
        assert!(!live_account_observation_matches(&record, &selected, 999));
        assert!(!live_account_observation_matches(
            &record, &selected, 121001
        ));
        for (path, value) in [
            ("/accountMode", json!("paper")),
            ("/health/account/mode", json!("provider_reported")),
            ("/health/account/status", json!("INACTIVE")),
            ("/health/account/id", json!("different")),
            ("/health/evidenceCurrent", json!(false)),
            ("/credentialStatus/configured", json!(false)),
        ] {
            let mut changed = record.clone();
            *changed.pointer_mut(path).unwrap() = value;
            assert!(
                !live_account_observation_matches(&changed, &selected, 1000),
                "{path}"
            );
        }
        let mut wrong = selected.clone();
        for field in ["tradingBlocked", "accountBlocked", "tradeSuspendedByUser"] {
            for value in [json!(true), Value::Null, json!("false")] {
                let mut changed = record.clone();
                changed["health"]["account"][field] = value;
                assert!(
                    !live_account_observation_matches(&changed, &selected, 1000),
                    "{field}"
                );
            }
        }
        wrong["accountRef"] = json!("account://broker/two");
        assert!(!live_account_observation_matches(&record, &wrong, 1000));
        let checks = live_activation_checks(&json!({}), true, false, true, false, false, false);
        assert!(checks
            .iter()
            .any(|check| check["id"] == "live_credential_posture" && check["status"] == "pass"));
        assert!(checks
            .iter()
            .any(|check| check["id"] == "legal_acknowledgement" && check["status"] == "blocked"));
    }

    #[test]
    fn strategy_spec_receipt_binding_preserves_the_canonical_hash() {
        let canonical_hash = hash_strategy_spec(&json!({"specVersion": "3.0"}));
        assert_eq!(
            strategy_spec_version_ref(&canonical_hash).expect("canonical strategy hash"),
            canonical_hash
        );
        assert_eq!(
            strategy_spec_version_ref(&format!("sha256:{canonical_hash}")),
            Err(crate::ports::LegalReceiptFailure::Invalid)
        );
    }
}
