// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    BrokerSubmissionPort, ClockPort, CredentialPort, FailureMode, ImmutablePutOutcome,
    PluginOperationPort, PluginOperationRequest, PluginOperationResponse, PluginPackagePort,
    PluginProcessSandboxPort, PluginRegistryPort, PortDescriptor, PortKind, SideEffectContext,
    StoragePort, VersionedPort,
};
use chrono::{DateTime, Datelike, Duration as ChronoDuration, TimeZone, Utc, Weekday};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const RECEIPTS_NS: &str = "plugin_operation_receipts";
const REQUESTS_NS: &str = "plugin_operation_requests";
const PREPARED_BINDINGS_NS: &str = "plugin_operation_prepared_bindings";
const BROKER_DISPATCH_NS: &str = "plugin_broker_dispatch_claims";
const BROKER_DENIAL_NS: &str = "plugin_broker_submission_denials";
const RECOVERY_REQUESTS_NS: &str = "plugin_broker_recovery_requests";
const RECOVERY_OUTCOMES_NS: &str = "plugin_broker_recovery_outcomes";
pub(crate) const RESET_NAMESPACES: &[&str] = &[
    REQUESTS_NS,
    PREPARED_BINDINGS_NS,
    RECEIPTS_NS,
    BROKER_DISPATCH_NS,
    BROKER_DENIAL_NS,
    RECOVERY_REQUESTS_NS,
    RECOVERY_OUTCOMES_NS,
];

pub struct LocalPluginOperations {
    storage: Arc<dyn StoragePort>,
    external: Option<crate::adapters::external_plugin_host::ExternalPluginHost>,
    broker_boundary: Option<Arc<dyn BrokerSubmissionPort>>,
}

impl LocalPluginOperations {
    pub fn new(storage: Arc<dyn StoragePort>) -> Self {
        Self {
            storage,
            external: None,
            broker_boundary: None,
        }
    }

    pub fn with_external_host(
        storage: Arc<dyn StoragePort>,
        registry: Arc<dyn PluginRegistryPort>,
        credentials: Arc<dyn CredentialPort>,
        packages: Arc<dyn PluginPackagePort>,
        clock: Arc<dyn ClockPort>,
        sandbox: Arc<dyn PluginProcessSandboxPort>,
    ) -> Self {
        Self {
            storage,
            external: Some(
                crate::adapters::external_plugin_host::ExternalPluginHost::new(
                    registry,
                    credentials,
                    packages,
                    clock,
                    sandbox,
                ),
            ),
            broker_boundary: None,
        }
    }

    pub fn with_broker_boundary(mut self, boundary: Arc<dyn BrokerSubmissionPort>) -> Self {
        self.broker_boundary = Some(boundary);
        self
    }

    fn record_terminal_denial(
        &self,
        key: &str,
        request_hash: &str,
        request: &PluginOperationRequest,
        reason: &str,
        context: &SideEffectContext,
    ) -> Result<(), String> {
        let denial = json!({
            "requestHash": request_hash,
            "correlationId": request.correlation_id,
            "state": "denied",
            "reason": reason,
        });
        if self
            .storage
            .put_json_if_absent(BROKER_DENIAL_NS, key, denial.clone(), context)?
            == ImmutablePutOutcome::AlreadyPresent
        {
            let existing = self
                .storage
                .get_json(BROKER_DENIAL_NS, key)?
                .ok_or_else(|| "plugin_order_denial_missing".to_string())?;
            if existing["requestHash"] != request_hash
                || existing["correlationId"] != request.correlation_id
            {
                return Err("plugin_operation_idempotency_conflict".to_string());
            }
        }
        Ok(())
    }
}

impl VersionedPort for LocalPluginOperations {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::Plugins, "local.plugin-operations")
            .for_profiles(&["local", "self_hosted"])
            .with_capabilities(&[
                "plugin.operation.invoke",
                "market_data.quote.read@1",
                "market_data.bars.read@1",
                "broker.order_submit.paper",
                "calendar.session.resolve@1",
            ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        vec![descriptor]
    }
}

impl PluginOperationPort for LocalPluginOperations {
    fn verify_broker_boundary(&self) -> Result<(), String> {
        if self.external.is_none() {
            return Err("external_broker_host_unavailable".into());
        }
        self.broker_boundary
            .as_ref()
            .ok_or_else(|| "broker_submission_boundary_unavailable".to_string())?
            .verify_available()
    }

    fn recover_ambiguous_broker_order(
        &self,
        plan: &crate::broker_submission::BrokerOrderRecoveryPlan,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        plan.revalidate()?;
        let key = context.idempotency_key.as_str();
        if key == plan.original_key {
            return Err("broker_recovery_distinct_key_required".into());
        }
        // Authenticate before even returning an existing original receipt.
        if let Some(stored) = self.storage.get_json(RECEIPTS_NS, &plan.original_key)? {
            let response = validated_stored_response(stored, &plan.lookup)?;
            crate::broker_submission::validate_order_identity(&plan.intent, &response)?;
            return Ok(response);
        }
        let record = json!({"originalKey":plan.original_key,
            "intentDigest":crate::spec::canonical_hash(&plan.intent)?,
            "preparedBinding":plan.binding});
        if self
            .storage
            .put_json_if_absent(RECOVERY_REQUESTS_NS, key, record.clone(), context)?
            == ImmutablePutOutcome::AlreadyPresent
        {
            if self.storage.get_json(RECOVERY_REQUESTS_NS, key)? != Some(record) {
                return Err("broker_recovery_idempotency_conflict".into());
            }
            // This read attempt may itself have lost its response. A new explicit
            // recovery key may observe again; never repeat the original submit.
            return Err("plugin_order_reconciliation_required".into());
        }
        let result = (|| {
            let host = self
                .external
                .as_ref()
                .ok_or("broker_recovery_host_unavailable")?;
            let prepared = host.prepare_broker_recovery(&plan.lookup, context)?;
            if prepared.binding != plan.binding {
                return Err("broker_recovery_prepared_binding_changed".into());
            }
            plan.revalidate()?;
            let mut response = host.execute_prepared(prepared)?;
            if response.correlation_id != plan.lookup.correlation_id {
                return Err("broker_recovery_correlation_mismatch".into());
            }
            crate::broker_submission::validate_order_identity(&plan.intent, &response)?;
            plan.revalidate()?;
            response.evidence_refs.retain(|r| {
                !r.starts_with("plugin-receipt:") && !r.starts_with("broker-recovery:")
            });
            response
                .evidence_refs
                .push(format!("plugin-receipt:{}", plan.original_key));
            response
                .evidence_refs
                .push(format!("broker-recovery:{key}"));
            Ok(response)
        })();
        let outcome = match &result {
            Ok(response) => {
                json!({"state":"found", "originalKey":plan.original_key, "response":response})
            }
            // Do not persist provider diagnostics or credentials in failure evidence.
            Err(_) => json!({"state":"unresolved", "originalKey":plan.original_key}),
        };
        if self
            .storage
            .put_json_if_absent(RECOVERY_OUTCOMES_NS, key, outcome.clone(), context)?
            == ImmutablePutOutcome::AlreadyPresent
            && self.storage.get_json(RECOVERY_OUTCOMES_NS, key)? != Some(outcome)
        {
            return Err("plugin_order_reconciliation_required".into());
        }
        let response =
            result.map_err(|_: String| "plugin_order_reconciliation_required".to_string())?;
        let value =
            serde_json::to_value(&response).map_err(|_| "broker_recovery_receipt_invalid")?;
        if self.storage.put_json_if_absent(
            RECEIPTS_NS,
            &plan.original_key,
            value.clone(),
            context,
        )? == ImmutablePutOutcome::AlreadyPresent
            && self.storage.get_json(RECEIPTS_NS, &plan.original_key)? != Some(value)
        {
            return Err("plugin_order_reconciliation_required".into());
        }
        Ok(response)
    }

    fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        validate_request(request)?;
        // An unavailable boundary has made no attempt. Do not create a
        // dispatch claim that would turn its next retry into an ambiguity.
        if is_live_submission(request) && self.broker_boundary.is_none() {
            return Err("plugin_operation_mode_unsupported".into());
        }
        let mut hash_request = serde_json::to_value(request)
            .map_err(|_| "plugin_operation_request_invalid".to_string())?;
        hash_request
            .as_object_mut()
            .expect("plugin request serializes as an object")
            .remove("correlationId");
        let receipt_key = context.idempotency_key.as_str();
        let request_hash = short_hash(
            serde_json::to_vec(&json!({
                "request": hash_request,
                "authority": context.authority,
            }))
            .map_err(|_| "plugin_operation_request_invalid".to_string())?
            .as_slice(),
        );
        let request_record = json!({
            "correlationId": request.correlation_id,
            "requestHash": request_hash,
            // Bounded non-secret provenance for subsequent receipt consumers.
            // Never copy the arbitrary operation input or credential material.
            "binding": {
                "schemaVersion": "tradeassembly.plugin_request_binding.v1",
                "operationId": request.operation_id,
                "capability": request.capability,
                "pluginInstanceRef": request.plugin_instance_ref,
                "pluginRef": request.plugin_ref,
                "manifestFingerprint": request.manifest_fingerprint,
                "capabilityGraphRevisionId": request.capability_graph_revision_id,
                "capabilityGraphFingerprint": request.capability_graph_fingerprint,
                "accountRef": request.account_ref,
                "mode": request.mode,
                "symbolHash": crate::spec::canonical_hash(&request.input["symbol"])
                    .map_err(|_| "plugin_request_binding_invalid".to_string())?,
            },
        });
        if self.storage.put_json_if_absent(
            REQUESTS_NS,
            receipt_key,
            request_record.clone(),
            context,
        )? == ImmutablePutOutcome::AlreadyPresent
        {
            let existing = self
                .storage
                .get_json(REQUESTS_NS, receipt_key)?
                .ok_or_else(|| "plugin_operation_request_missing".to_string())?;
            if existing["requestHash"] != request_hash
                || existing["correlationId"]
                    .as_str()
                    .is_some_and(|value| value != request.correlation_id)
            {
                return Err("plugin_operation_idempotency_conflict".to_string());
            }
        }
        if let Some(stored) = self.storage.get_json(RECEIPTS_NS, receipt_key)? {
            return validated_stored_response(stored, request);
        }

        if let Some(denial) = self.storage.get_json(BROKER_DENIAL_NS, receipt_key)? {
            if denial["requestHash"] != request_hash
                || denial["correlationId"] != request.correlation_id
            {
                return Err("plugin_operation_idempotency_conflict".to_string());
            }
            return Err("plugin_order_submission_denied".into());
        }

        let broker_submission = request.operation_id.starts_with("broker.")
            || request.capability.starts_with("broker.");
        let live_submission = is_live_submission(request);
        if broker_submission {
            // A request record is not an exclusive dispatch claim. A matching
            // concurrent call, or a restart after an uncertain effect, must not
            // submit again while the first response is absent. Never delete this
            // claim merely because dispatch/receipt persistence returned an error.
            let claim = json!({"requestHash":request_hash,"correlationId":request.correlation_id,"state":"claimed"});
            if self
                .storage
                .put_json_if_absent(BROKER_DISPATCH_NS, receipt_key, claim, context)?
                == ImmutablePutOutcome::AlreadyPresent
            {
                return match self.storage.get_json(RECEIPTS_NS, receipt_key)? {
                    Some(stored) => validated_stored_response(stored, request),
                    None => Err("plugin_order_reconciliation_required".into()),
                };
            }
        }

        let external_prepared = match (request.plugin_ref.as_str(), request.operation_id.as_str()) {
            ("tradeassembly.simbroker", "marketdata.quote.read")
            | ("tradeassembly.simbroker", "marketdata.bars.read")
            | ("tradeassembly.simbroker", "marketdata.bars.read_v1")
            | ("tradeassembly.local-data", "marketdata.bars.read")
            | ("tradeassembly.local-data", "marketdata.bars.read_v1")
            | ("tradeassembly.simbroker", "broker.paper_order_submit")
            | ("tradeassembly.core-runtime", "calendar.session.resolve_v1") => None,
            _ => {
                let external = self
                    .external
                    .as_ref()
                    .ok_or_else(|| "plugin_operation_unsupported".to_string());
                match external.and_then(|external| {
                    let prepared = external.prepare(request, context)?;
                    let binding = serde_json::to_value(&prepared.binding)
                        .map_err(|_| "plugin_prepared_binding_invalid".to_string())?;
                    if self.storage.put_json_if_absent(
                        PREPARED_BINDINGS_NS,
                        receipt_key,
                        binding.clone(),
                        context,
                    )? == ImmutablePutOutcome::AlreadyPresent
                        && self.storage.get_json(PREPARED_BINDINGS_NS, receipt_key)?
                            != Some(binding)
                    {
                        return Err("plugin_prepared_binding_changed".into());
                    }
                    Ok(prepared)
                }) {
                    Ok(prepared) => Some(prepared),
                    Err(_) if broker_submission => {
                        self.record_terminal_denial(
                            receipt_key,
                            &request_hash,
                            request,
                            "prepare_denied",
                            context,
                        )?;
                        return Err("plugin_order_submission_denied".into());
                    }
                    Err(error) => return Err(error),
                }
            }
        };

        let permit = if live_submission {
            let Some(boundary) = self.broker_boundary.as_ref() else {
                self.record_terminal_denial(
                    receipt_key,
                    &request_hash,
                    request,
                    "boundary_unavailable",
                    context,
                )?;
                return Err("plugin_order_submission_denied".into());
            };
            let prepared_binding = external_prepared
                .as_ref()
                .ok_or_else(|| "plugin_live_prepared_binding_missing".to_string())?;
            match boundary.admit(request, context, &prepared_binding.binding) {
                Ok(permit) => Some(permit),
                Err(_) => {
                    self.record_terminal_denial(
                        receipt_key,
                        &request_hash,
                        request,
                        "admission_denied",
                        context,
                    )?;
                    return Err("plugin_order_submission_denied".into());
                }
            }
        } else {
            None
        };

        let response = match (request.plugin_ref.as_str(), request.operation_id.as_str()) {
            ("tradeassembly.simbroker", "marketdata.quote.read") => quote(request),
            ("tradeassembly.simbroker", "marketdata.bars.read")
            | ("tradeassembly.simbroker", "marketdata.bars.read_v1")
            | ("tradeassembly.local-data", "marketdata.bars.read")
            | ("tradeassembly.local-data", "marketdata.bars.read_v1") => bar(request),
            ("tradeassembly.simbroker", "broker.paper_order_submit") => paper_order(request),
            ("tradeassembly.core-runtime", "calendar.session.resolve_v1") => {
                calendar_session(request)
            }
            _ => self
                .external
                .as_ref()
                .ok_or_else(|| "plugin_operation_unsupported".to_string())?
                .execute_prepared(external_prepared.expect("external invocation prepared")),
        };
        let mut response = match response {
            Ok(response) => response,
            Err(_) if broker_submission => {
                return Err("plugin_order_reconciliation_required".into())
            }
            Err(error) => return Err(error),
        };
        if !response.reconciliation_required {
            if let (Some(boundary), Some(permit)) = (self.broker_boundary.as_ref(), permit.as_ref())
            {
                if boundary.complete(permit, &response).is_err() {
                    return Err("plugin_order_reconciliation_required".into());
                }
            }
        }
        // Runtime-owned locator for immutable request/preparation/response evidence.
        // Plugins cannot nominate a different receipt in this reserved namespace.
        response
            .evidence_refs
            .retain(|reference| !reference.starts_with("plugin-receipt:"));
        response
            .evidence_refs
            .push(format!("plugin-receipt:{receipt_key}"));
        let persisted = (|| {
            let serialized = serde_json::to_value(&response)
                .map_err(|_| "plugin_operation_response_invalid".to_string())?;
            match self
                .storage
                .put_json_if_absent(RECEIPTS_NS, receipt_key, serialized, context)?
            {
                ImmutablePutOutcome::Created => Ok(response),
                ImmutablePutOutcome::AlreadyPresent => self
                    .storage
                    .get_json(RECEIPTS_NS, receipt_key)?
                    .ok_or_else(|| "plugin_operation_receipt_missing".to_string())
                    .and_then(|stored| validated_stored_response(stored, request)),
            }
        })();
        if broker_submission {
            // The sink already ran. A missing/invalid durable receipt cannot be
            // reported as an ordinary failed submission that is safe to retry.
            persisted.map_err(|_| "plugin_order_reconciliation_required".to_string())
        } else {
            persisted
        }
    }
}

fn validated_stored_response(
    stored: Value,
    request: &PluginOperationRequest,
) -> Result<PluginOperationResponse, String> {
    let mut response: PluginOperationResponse = serde_json::from_value(stored)
        .map_err(|_| "plugin_operation_receipt_invalid".to_string())?;
    if response.correlation_id.is_empty() {
        response.correlation_id = request.correlation_id.clone();
    }
    if response.correlation_id != request.correlation_id {
        return Err("plugin_operation_correlation_conflict".to_string());
    }
    Ok(response)
}

fn validate_request(request: &PluginOperationRequest) -> Result<(), String> {
    if request.operation_id == "broker.order_lookup"
        || request.capability == "broker.order_lookup.live"
    {
        return Err("broker_order_recovery_plan_required".into());
    }
    if [
        request.correlation_id.as_str(),
        request.plugin_instance_ref.as_str(),
        request.plugin_ref.as_str(),
        request.manifest_fingerprint.as_str(),
        request.operation_id.as_str(),
        request.capability.as_str(),
        request.capability_graph_revision_id.as_str(),
        request.capability_graph_fingerprint.as_str(),
        request.mode.as_str(),
        request.purpose.as_str(),
    ]
    .iter()
    .any(|value| value.trim().is_empty())
    {
        return Err("plugin_operation_request_invalid".to_string());
    }
    // The declared capability is part of the boundary too: a renamed operation
    // must not disguise a broker call as a market/account read.
    let broker_operation =
        request.operation_id.starts_with("broker.") || request.capability.starts_with("broker.");
    if broker_operation
        && [
            request.strategy_id.as_str(),
            request.strategy_version_id.as_str(),
            request.strategy_spec_hash.as_str(),
            request.activation_id.as_str(),
            request.attempt_id.as_str(),
            request.evaluation_tick_id.as_str(),
        ]
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err("plugin_operation_execution_context_required".to_string());
    }
    if !matches!(
        request.mode.as_str(),
        "paper" | "live" | "research" | "backtest"
    ) {
        return Err("plugin_operation_mode_unsupported".to_string());
    }
    // Live reads are needed to observe account/data posture before admission.
    // Live submissions remain closed until the broker-boundary mandate/PEP is
    // installed; supporting reads must not silently open order submission.
    if broker_operation
        && ((request.mode != "paper" && !is_live_submission(request))
            || (request.capability == "broker.order_submit.live" && !is_live_submission(request))
            || (request.operation_id == "broker.live_order_submit" && !is_live_submission(request)))
    {
        return Err("plugin_operation_mode_unsupported".to_string());
    }
    if request.plugin_ref == "tradeassembly.simbroker"
        && broker_operation
        && request.mode != "paper"
    {
        return Err("plugin_operation_mode_unsupported".to_string());
    }
    if request.timeout_ms == 0 || request.fencing_token.is_some_and(|token| token < 0) {
        return Err("plugin_operation_request_invalid".to_string());
    }
    if contains_secret_shaped_field(&request.input) {
        return Err("plugin_operation_raw_credential_forbidden".to_string());
    }
    Ok(())
}

fn is_live_submission(request: &PluginOperationRequest) -> bool {
    request.mode == "live"
        && request.capability == "broker.order_submit.live"
        && request.operation_id == "broker.live_order_submit"
        && request.purpose == "live_order_submission"
}

fn contains_secret_shaped_field(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            let normalized = key.to_ascii_lowercase();
            matches!(
                normalized.as_str(),
                "apikey"
                    | "api_key"
                    | "apisecret"
                    | "api_secret"
                    | "clientsecret"
                    | "client_secret"
                    | "password"
                    | "token"
                    | "access_token"
                    | "refresh_token"
                    | "credential"
                    | "credentials"
            ) || contains_secret_shaped_field(value)
        }),
        Value::Array(values) => values.iter().any(contains_secret_shaped_field),
        _ => false,
    }
}

fn quote(request: &PluginOperationRequest) -> Result<PluginOperationResponse, String> {
    let symbol = required_string(&request.input, "symbol")?;
    let sequence = required_u64(&request.input, "sequence")?;
    let price_micros = fixture_price_micros(symbol, sequence)?;
    response(
        request,
        "schema://market-data/quote@1",
        json!({
            "symbol": symbol,
            "bidMicros": price_micros - 500,
            "askMicros": price_micros + 500,
            "priceMicros": price_micros,
            "sequence": sequence,
            "sourceClass": "deterministic_fixture",
        }),
        sequence,
    )
}

fn bar(request: &PluginOperationRequest) -> Result<PluginOperationResponse, String> {
    let symbol = required_string(&request.input, "symbol")?;
    let sequence = required_u64(&request.input, "sequence")?;
    let close = fixture_price_micros(symbol, sequence)?;
    response(
        request,
        "schema://market-data/bar@1",
        json!({
            "symbol": symbol,
            "barIndex": sequence,
            "openMicros": close - 10_000,
            "highMicros": close + 25_000,
            "lowMicros": close - 25_000,
            "closeMicros": close,
            "volumeMicros": 100_000_000_000_i64,
            "timeframe": request.input.get("timeframe").and_then(Value::as_str).unwrap_or("1m"),
            "sourceClass": "deterministic_fixture",
        }),
        sequence,
    )
}

fn paper_order(request: &PluginOperationRequest) -> Result<PluginOperationResponse, String> {
    let symbol = required_string(&request.input, "symbol")?;
    let side = required_string(&request.input, "side")?;
    if !matches!(side, "buy" | "sell") {
        return Err("plugin_operation_input_invalid".to_string());
    }
    let quantity_micros = required_i64(&request.input, "quantityMicros")?;
    if quantity_micros <= 0 {
        return Err("plugin_operation_input_invalid".to_string());
    }
    let sequence = required_u64(&request.input, "sequence")?;
    let fill_price_micros = required_i64(&request.input, "fillPriceMicros")?;
    if fill_price_micros <= 0 {
        return Err("plugin_operation_input_invalid".to_string());
    }
    response(
        request,
        "schema://broker/order-receipt@1",
        json!({
            "providerOrderId": format!(
                "sim-order-{}",
                short_hash(
                    format!("{}:{}", request.activation_id, request.evaluation_tick_id).as_bytes()
                )
            ),
            "clientOrderId": request.input.get("clientOrderId").and_then(Value::as_str).unwrap_or(&request.evaluation_tick_id),
            "symbol": symbol,
            "side": side,
            "quantityMicros": quantity_micros,
            "filledQuantityMicros": quantity_micros,
            "fillPriceMicros": fill_price_micros,
            "status": "filled",
            "sourceClass": "deterministic_simulation",
        }),
        sequence,
    )
}

fn calendar_session(request: &PluginOperationRequest) -> Result<PluginOperationResponse, String> {
    let calendar_ref = required_string(&request.input, "calendarRef")?;
    let observed_at_ms = required_i64(&request.input, "observedAtMs")?;
    let sequence = required_u64(&request.input, "sequence")?;
    let timestamp = Utc
        .timestamp_millis_opt(observed_at_ms)
        .single()
        .ok_or_else(|| "plugin_operation_input_invalid".to_string())?;
    let payload = match calendar_ref {
        "CRYPTO_24X7" | "CONTINUOUS" => json!({
            "calendarRef": calendar_ref,
            "observedAtMs": observed_at_ms,
            "eligible": true,
            "sessionState": "open",
            "sourceClass": "deterministic_reference_calendar",
            "calendarModel": "continuous_reference.v1",
        }),
        "XNYS" | "NYSE" | "US_EQUITIES" => xnys_2026_session(timestamp, calendar_ref)?,
        _ => return Err("calendar_ref_unsupported".to_string()),
    };
    response_at(
        request,
        "schema://calendar/session-resolution@1",
        payload,
        sequence,
        observed_at_ms,
    )
}

const XNYS_REFERENCE_YEAR: i32 = 2026;
const XNYS_CALENDAR_MODEL: &str = "xnys_equities_regular_session.2026.v1";
const XNYS_CALENDAR_SOURCE: &str = "nyse_declared_equities_schedule.2026.v1";
const XNYS_TIMEZONE: &str = "America/New_York";

fn xnys_2026_session(timestamp: DateTime<Utc>, calendar_ref: &str) -> Result<Value, String> {
    let local = us_eastern_local(timestamp)?;
    if local.year() != XNYS_REFERENCE_YEAR {
        return Err("calendar_coverage_unsupported".to_string());
    }

    let is_holiday = xnys_holiday(local.month(), local.day());
    let is_early_close = xnys_early_close(local.month(), local.day());
    let opens_at_ms = xnys_session_boundary_ms(local.year(), local.month(), local.day(), 9, 30)?;
    let closes_at_ms = xnys_session_boundary_ms(
        local.year(),
        local.month(),
        local.day(),
        if is_early_close { 13 } else { 16 },
        0,
    )?;
    let is_session_day = !matches!(local.weekday(), Weekday::Sat | Weekday::Sun) && !is_holiday;
    let eligible = is_session_day
        && timestamp.timestamp_millis() >= opens_at_ms
        && timestamp.timestamp_millis() < closes_at_ms;
    let (next_transition_kind, next_transition_at_ms) = if eligible {
        ("close", closes_at_ms)
    } else {
        let next_open = next_xnys_open(timestamp, local)?;
        ("open", next_open)
    };

    Ok(json!({
        "calendarId": "XNYS",
        "calendarRef": calendar_ref,
        "calendarModel": XNYS_CALENDAR_MODEL,
        "calendarSource": XNYS_CALENDAR_SOURCE,
        "sourceClass": "versioned_declared_reference_calendar",
        "sessionScope": "xnys_equities_regular_0930_1600_et",
        "timezone": XNYS_TIMEZONE,
        "observedAtMs": timestamp.timestamp_millis(),
        "eligible": eligible,
        "sessionState": if eligible { "open" } else { "closed" },
        "nextTransitionKind": next_transition_kind,
        "nextTransitionAtMs": next_transition_at_ms,
        "regularHoursOpenAtMs": opens_at_ms,
        "regularHoursCloseAtMs": closes_at_ms,
        "holiday": is_holiday,
        "earlyClose": is_early_close,
    }))
}

fn us_eastern_local(timestamp: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let utc_offset_hours = if us_eastern_is_dst(timestamp)? { 4 } else { 5 };
    Ok(timestamp - ChronoDuration::hours(utc_offset_hours))
}

fn us_eastern_is_dst(timestamp: DateTime<Utc>) -> Result<bool, String> {
    let year = timestamp.year();
    let dst_start = nth_sunday_utc(year, 3, 2, 7)?;
    let dst_end = nth_sunday_utc(year, 11, 1, 6)?;
    Ok(timestamp >= dst_start && timestamp < dst_end)
}

fn xnys_session_boundary_ms(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
) -> Result<i64, String> {
    let local_noon_utc = Utc
        .with_ymd_and_hms(year, month, day, 12, 0, 0)
        .single()
        .ok_or_else(|| "calendar_timestamp_invalid".to_string())?;
    let utc_offset_hours = if us_eastern_is_dst(local_noon_utc)? {
        4
    } else {
        5
    };
    Utc.with_ymd_and_hms(year, month, day, hour + utc_offset_hours, minute, 0)
        .single()
        .map(|timestamp| timestamp.timestamp_millis())
        .ok_or_else(|| "calendar_timestamp_invalid".to_string())
}

fn next_xnys_open(timestamp: DateTime<Utc>, local: DateTime<Utc>) -> Result<i64, String> {
    let mut candidate = local
        .date_naive()
        .succ_opt()
        .ok_or_else(|| "calendar_coverage_unsupported".to_string())?;
    let today_open = xnys_session_boundary_ms(local.year(), local.month(), local.day(), 9, 30)?;
    if timestamp.timestamp_millis() < today_open {
        candidate = local.date_naive();
    }

    while candidate.year() == XNYS_REFERENCE_YEAR {
        if !matches!(candidate.weekday(), Weekday::Sat | Weekday::Sun)
            && !xnys_holiday(candidate.month(), candidate.day())
        {
            return xnys_session_boundary_ms(
                candidate.year(),
                candidate.month(),
                candidate.day(),
                9,
                30,
            );
        }
        candidate = candidate
            .succ_opt()
            .ok_or_else(|| "calendar_coverage_unsupported".to_string())?;
    }
    Err("calendar_coverage_unsupported".to_string())
}

fn xnys_holiday(month: u32, day: u32) -> bool {
    matches!(
        (month, day),
        (1, 1)
            | (1, 19)
            | (2, 16)
            | (4, 3)
            | (5, 25)
            | (6, 19)
            | (7, 3)
            | (9, 7)
            | (11, 26)
            | (12, 25)
    )
}

fn xnys_early_close(month: u32, day: u32) -> bool {
    matches!((month, day), (11, 27) | (12, 24))
}

fn nth_sunday_utc(
    year: i32,
    month: u32,
    occurrence: u32,
    utc_hour: u32,
) -> Result<DateTime<Utc>, String> {
    let first = Utc
        .with_ymd_and_hms(year, month, 1, utc_hour, 0, 0)
        .single()
        .ok_or_else(|| "calendar_timestamp_invalid".to_string())?;
    let days_to_first_sunday = (7 - first.weekday().num_days_from_sunday()) % 7;
    Ok(first
        + ChronoDuration::days(i64::from(
            days_to_first_sunday + 7 * occurrence.saturating_sub(1),
        )))
}

fn response(
    request: &PluginOperationRequest,
    schema_ref: &str,
    payload: Value,
    sequence: u64,
) -> Result<PluginOperationResponse, String> {
    let observed_at_ms = i64::try_from(sequence)
        .map_err(|_| "plugin_operation_input_invalid".to_string())?
        .checked_mul(60_000)
        .ok_or_else(|| "plugin_operation_input_invalid".to_string())?;
    response_at(request, schema_ref, payload, sequence, observed_at_ms)
}

fn response_at(
    request: &PluginOperationRequest,
    schema_ref: &str,
    payload: Value,
    sequence: u64,
    observed_at_ms: i64,
) -> Result<PluginOperationResponse, String> {
    let source_event_id = format!(
        "plugin-event-{}",
        short_hash(format!("{}:{sequence}", request.evaluation_tick_id).as_bytes())
    );
    let content_hash = short_hash(
        serde_json::to_vec(&json!({
            "schemaRef": schema_ref,
            "payload": payload,
            "observedAtMs": observed_at_ms,
            "sourceEventId": source_event_id,
        }))
        .map_err(|_| "plugin_operation_response_invalid".to_string())?
        .as_slice(),
    );
    let provider_outcome_id = payload
        .get("providerOrderId")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(PluginOperationResponse {
        correlation_id: request.correlation_id.clone(),
        schema_ref: schema_ref.to_string(),
        payload,
        observed_at_ms,
        source_event_id,
        content_hash,
        freshness_state: "fresh".to_string(),
        evidence_refs: request.evidence_refs.clone(),
        deterministic: true,
        replayable: true,
        provider_outcome_id,
        reconciliation_required: false,
    })
}

fn fixture_price_micros(symbol: &str, sequence: u64) -> Result<i64, String> {
    if symbol == "BTC/USD" {
        let movement = i64::try_from(sequence.saturating_sub(1) % 20)
            .map_err(|_| "plugin_operation_input_invalid".to_string())?
            .checked_mul(1_000_000)
            .ok_or_else(|| "plugin_operation_input_invalid".to_string())?;
        return 101_000_000_i64
            .checked_sub(movement)
            .ok_or_else(|| "plugin_operation_input_invalid".to_string());
    }
    let seed = Sha256::digest(symbol.as_bytes());
    let base = 1_000_000_i64
        .checked_add(i64::from(seed[0]) * 10_000)
        .ok_or_else(|| "plugin_operation_input_invalid".to_string())?;
    let movement = i64::try_from(sequence % 1_000)
        .map_err(|_| "plugin_operation_input_invalid".to_string())?
        .checked_mul(50_000)
        .ok_or_else(|| "plugin_operation_input_invalid".to_string())?;
    base.checked_add(movement)
        .ok_or_else(|| "plugin_operation_input_invalid".to_string())
}

fn required_string<'a>(input: &'a Value, field: &str) -> Result<&'a str, String> {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "plugin_operation_input_invalid".to_string())
}

fn required_u64(input: &Value, field: &str) -> Result<u64, String> {
    input
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| "plugin_operation_input_invalid".to_string())
}

fn required_i64(input: &Value, field: &str) -> Result<i64, String> {
    input
        .get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| "plugin_operation_input_invalid".to_string())
}

fn short_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))[..24].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation_id: &str) -> PluginOperationRequest {
        PluginOperationRequest {
            correlation_id: "correlation-1".to_string(),
            plugin_instance_ref: "alpaca-paper".to_string(),
            plugin_ref: "tradeassembly.alpaca".to_string(),
            manifest_fingerprint: "manifest-digest".to_string(),
            operation_id: operation_id.to_string(),
            capability: "broker.order_submit.paper".to_string(),
            capability_graph_revision_id: "binding-1".to_string(),
            capability_graph_fingerprint: "binding-fingerprint".to_string(),
            strategy_id: String::new(),
            strategy_version_id: String::new(),
            strategy_spec_hash: String::new(),
            activation_id: String::new(),
            attempt_id: String::new(),
            evaluation_tick_id: String::new(),
            mode: "paper".to_string(),
            purpose: "paper_trading".to_string(),
            account_ref: Some("brokerage-account:example".to_string()),
            timeout_ms: 1_000,
            fencing_token: Some(1),
            input: json!({}),
            evidence_refs: Vec::new(),
        }
    }

    #[test]
    fn broker_operation_requires_complete_execution_context() {
        assert_eq!(
            validate_request(&request("broker.paper_order_submit")),
            Err("plugin_operation_execution_context_required".to_string())
        );

        let mut complete = request("broker.paper_order_submit");
        complete.strategy_id = "strategy-1".to_string();
        complete.strategy_version_id = "version-1".to_string();
        complete.strategy_spec_hash = "spec-hash".to_string();
        complete.activation_id = "activation-1".to_string();
        complete.attempt_id = "attempt-1".to_string();
        complete.evaluation_tick_id = "tick-1".to_string();
        assert_eq!(validate_request(&complete), Ok(()));
    }

    #[test]
    fn read_operation_does_not_require_execution_context() {
        let mut read = request("account.health");
        read.capability = "account.health".to_string();
        read.purpose = "account_health".to_string();
        read.fencing_token = None;
        for mode in ["paper", "live", "research"] {
            read.mode = mode.to_string();
            assert_eq!(validate_request(&read), Ok(()), "{mode}");
        }
    }

    #[test]
    fn broker_operation_rejects_non_paper_mode() {
        let mut broker = request("broker.paper_order_submit");
        broker.strategy_id = "strategy-1".to_string();
        broker.strategy_version_id = "version-1".to_string();
        broker.strategy_spec_hash = "spec-hash".to_string();
        broker.activation_id = "activation-1".to_string();
        broker.attempt_id = "attempt-1".to_string();
        broker.evaluation_tick_id = "tick-1".to_string();
        broker.mode = "research".to_string();
        assert_eq!(
            validate_request(&broker),
            Err("plugin_operation_mode_unsupported".to_string())
        );
    }

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid fixture timestamp")
    }

    #[test]
    fn xnys_2026_declares_versioned_session_metadata() {
        let session = xnys_2026_session(utc(2026, 1, 2, 14, 30), "XNYS").expect("session");

        assert_eq!(session["calendarId"], "XNYS");
        assert_eq!(session["calendarModel"], XNYS_CALENDAR_MODEL);
        assert_eq!(session["calendarSource"], XNYS_CALENDAR_SOURCE);
        assert_eq!(
            session["sessionScope"],
            "xnys_equities_regular_0930_1600_et"
        );
        assert_eq!(session["timezone"], XNYS_TIMEZONE);
        assert_eq!(session["sessionState"], "open");
        assert_eq!(session["eligible"], true);
        assert_eq!(session["nextTransitionKind"], "close");
        assert_eq!(
            session["nextTransitionAtMs"],
            utc(2026, 1, 2, 21, 0).timestamp_millis()
        );
        assert_eq!(session["holiday"], false);
        assert_eq!(session["earlyClose"], false);
    }

    #[test]
    fn xnys_2026_weekends_holidays_and_dst_fail_closed_until_next_open() {
        let weekend = xnys_2026_session(utc(2026, 6, 20, 15, 0), "XNYS").expect("session");
        assert_eq!(weekend["sessionState"], "closed");
        assert_eq!(weekend["eligible"], false);
        assert_eq!(weekend["nextTransitionKind"], "open");
        assert_eq!(
            weekend["nextTransitionAtMs"],
            utc(2026, 6, 22, 13, 30).timestamp_millis()
        );

        let good_friday = xnys_2026_session(utc(2026, 4, 3, 16, 0), "XNYS").expect("session");
        assert_eq!(good_friday["holiday"], true);
        assert_eq!(good_friday["eligible"], false);
        assert_eq!(
            good_friday["nextTransitionAtMs"],
            utc(2026, 4, 6, 13, 30).timestamp_millis()
        );

        let before_dst = xnys_2026_session(utc(2026, 3, 6, 14, 30), "XNYS").expect("session");
        let after_dst = xnys_2026_session(utc(2026, 3, 9, 13, 30), "XNYS").expect("session");
        assert_eq!(before_dst["eligible"], true);
        assert_eq!(after_dst["eligible"], true);
    }

    #[test]
    fn xnys_2026_only_declared_early_closes_end_at_one_pm_eastern() {
        let july_regular = xnys_2026_session(utc(2026, 7, 2, 17, 30), "XNYS").expect("session");
        assert_eq!(july_regular["earlyClose"], false);
        assert_eq!(july_regular["eligible"], true);
        assert_eq!(
            july_regular["regularHoursCloseAtMs"],
            utc(2026, 7, 2, 20, 0).timestamp_millis()
        );

        let thanksgiving = xnys_2026_session(utc(2026, 11, 27, 17, 59), "XNYS").expect("session");
        assert_eq!(thanksgiving["earlyClose"], true);
        assert_eq!(thanksgiving["eligible"], true);
        assert_eq!(
            thanksgiving["regularHoursCloseAtMs"],
            utc(2026, 11, 27, 18, 0).timestamp_millis()
        );

        let christmas_eve = xnys_2026_session(utc(2026, 12, 24, 18, 0), "XNYS").expect("session");
        assert_eq!(christmas_eve["earlyClose"], true);
        assert_eq!(christmas_eve["eligible"], false);
        assert_eq!(
            christmas_eve["nextTransitionAtMs"],
            utc(2026, 12, 28, 14, 30).timestamp_millis()
        );
    }

    #[test]
    fn xnys_2026_rejects_undeclared_coverage() {
        assert_eq!(
            xnys_2026_session(utc(2027, 1, 4, 14, 30), "XNYS"),
            Err("calendar_coverage_unsupported".to_string())
        );
        assert_eq!(
            xnys_2026_session(utc(2026, 12, 31, 22, 0), "XNYS"),
            Err("calendar_coverage_unsupported".to_string())
        );
    }
}
