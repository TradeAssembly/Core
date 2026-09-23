// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::{
    ClockPort, CredentialPort, PluginOperationRequest, PluginOperationResponse, PluginPackagePort,
    PluginProcessSandboxPort, PluginRegistryPort, PluginSandboxRequest, SideEffectContext,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::BufReader;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tradeassembly_plugin_sdk::{
    read_response, write_request, EphemeralCredentialGrant, PluginPackageDescriptor, PluginRequest,
    PluginResponse, Reconciliation, RequestContext, RequestMetadata, RequestPayload,
    ResponseStatus, DEFAULT_MAX_ENVELOPE_BYTES, SDK_VERSION, WIRE_CONTRACT_VERSION,
};

pub struct ExternalPluginHost {
    registry: Arc<dyn PluginRegistryPort>,
    credentials: Arc<dyn CredentialPort>,
    packages: Arc<dyn PluginPackagePort>,
    clock: Arc<dyn ClockPort>,
    sandbox: Arc<dyn PluginProcessSandboxPort>,
}

pub(crate) struct PreparedPluginInvocation {
    pub(crate) binding: crate::ports::broker_submission::BrokerPreparedBinding,
    request: PluginOperationRequest,
    package: crate::ports::InstalledPluginPackage,
    operation: Value,
    credential_values: std::collections::BTreeMap<String, String>,
    wire_request: PluginRequest,
    allowed_domains: Vec<String>,
    timeout_ms: u64,
}

impl ExternalPluginHost {
    pub fn new(
        registry: Arc<dyn PluginRegistryPort>,
        credentials: Arc<dyn CredentialPort>,
        packages: Arc<dyn PluginPackagePort>,
        clock: Arc<dyn ClockPort>,
        sandbox: Arc<dyn PluginProcessSandboxPort>,
    ) -> Self {
        Self {
            registry,
            credentials,
            packages,
            clock,
            sandbox,
        }
    }

    pub fn invoke(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PluginOperationResponse, String> {
        self.execute_prepared(self.prepare(request, context)?)
    }

    pub(crate) fn prepare(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PreparedPluginInvocation, String> {
        let instance = self
            .registry
            .get_instance(&request.plugin_instance_ref)?
            .ok_or_else(|| "plugin_instance_not_found".to_string())?;
        validate_instance(request, &instance)?;

        let package_digest = required_string(&instance, "activePackageSha256")?;
        let package = self
            .packages
            .get(package_digest)?
            .ok_or_else(|| "plugin_package_not_installed".to_string())?;

        let manifest_record = self
            .registry
            .get_manifest(&request.plugin_ref)?
            .ok_or_else(|| "plugin_manifest_not_installed".to_string())?;
        let manifest_digest = validate_package_binding(request, &package, &manifest_record)?;
        let operation = declared_operation(&manifest_record["manifest"], request)?.clone();
        let timeout_ms = operation_timeout_ms(&manifest_record["manifest"], request.timeout_ms)?;
        let credential_required = operation["credentialGrantRequired"]
            .as_bool()
            .unwrap_or(false);
        let credential_ref = required_string(&instance, "credentialRef")?;
        let credential_values = if credential_required {
            self.credentials
                .resolve_fields(credential_ref)?
                .filter(|values| !values.is_empty())
                .ok_or_else(|| "plugin_credentials_missing".to_string())?
        } else {
            Default::default()
        };

        let wire_request = build_wire_request(
            request,
            context,
            &instance,
            &package.package_sha256,
            manifest_digest,
            timeout_ms,
            credential_values.clone(),
        )?;
        let allowed_domains = allowed_network_domains(&manifest_record["manifest"], &instance)?;
        Ok(PreparedPluginInvocation {
            binding: crate::ports::broker_submission::BrokerPreparedBinding {
                plugin_instance_ref: request.plugin_instance_ref.clone(),
                plugin_ref: request.plugin_ref.clone(),
                package_sha256: package.package_sha256.clone(),
                manifest_fingerprint: request.manifest_fingerprint.clone(),
                configuration_digest: crate::spec::canonical_hash(&instance["configuration"])
                    .map_err(|_| "plugin_configuration_digest_invalid".to_string())?,
                credential_ref: credential_ref.to_string(),
                credential_generation: crate::live_execution_checks::credential_revision(&instance),
            },
            request: request.clone(),
            package,
            operation,
            credential_values,
            wire_request,
            allowed_domains,
            timeout_ms,
        })
    }

    pub(crate) fn execute_prepared(
        &self,
        prepared: PreparedPluginInvocation,
    ) -> Result<PluginOperationResponse, String> {
        let wire_response = execute(
            self.sandbox.as_ref(),
            &prepared.package.executable_path,
            &prepared.package.install_root,
            prepared.allowed_domains,
            &prepared.wire_request,
            prepared.timeout_ms,
        )?;
        crate::adapters::plugin_output_policy::validate_plugin_response_policy(
            &prepared.wire_request,
            &wire_response,
            &prepared.operation,
            &prepared.credential_values,
        )?;
        validate_response_schema_document(&prepared.package, &wire_response)?;
        response_from_wire(
            &prepared.request,
            &prepared.wire_request,
            wire_response,
            &prepared.operation,
            self.clock.now_ms(),
        )
    }

    pub(crate) fn prepare_broker_recovery(
        &self,
        request: &PluginOperationRequest,
        context: &SideEffectContext,
    ) -> Result<PreparedPluginInvocation, String> {
        if request.operation_id != "broker.order_lookup"
            || request.capability != "broker.order_lookup.live"
            || request.purpose != "recovery"
            || request.mode != "live"
        {
            return Err("broker_recovery_operation_invalid".into());
        }
        let prepared = self.prepare(request, context)?;
        if prepared.operation["effect"] != "read" {
            return Err("broker_recovery_operation_not_readonly".into());
        }
        Ok(prepared)
    }
}

fn validate_response_schema_document(
    package: &crate::ports::InstalledPluginPackage,
    response: &PluginResponse,
) -> Result<(), String> {
    let descriptor: PluginPackageDescriptor = serde_json::from_value(package.descriptor.clone())
        .map_err(|_| "plugin_package_descriptor_invalid".to_string())?;
    let schema = descriptor
        .response_schemas
        .iter()
        .find(|schema| schema.schema_ref == response.response_schema)
        .ok_or_else(|| "plugin_response_schema_missing".to_string())?;
    let root = std::fs::canonicalize(&package.install_root)
        .map_err(|_| "plugin_response_schema_missing".to_string())?;
    let path = std::fs::canonicalize(root.join(&schema.document.path))
        .map_err(|_| "plugin_response_schema_missing".to_string())?;
    if !path.starts_with(&root) {
        return Err("plugin_response_schema_missing".to_string());
    }
    let bytes = std::fs::read(path).map_err(|_| "plugin_response_schema_missing".to_string())?;
    if format!("{:x}", Sha256::digest(&bytes)) != schema.document.sha256 {
        return Err("plugin_response_schema_digest_mismatch".to_string());
    }
    let schema: Value =
        serde_json::from_slice(&bytes).map_err(|_| "plugin_response_schema_invalid".to_string())?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|_| "plugin_response_schema_invalid".to_string())?;
    if !validator.is_valid(&response.payload) {
        return Err("plugin_response_payload_schema_mismatch".to_string());
    }
    Ok(())
}

fn validate_package_binding<'a>(
    request: &PluginOperationRequest,
    package: &crate::ports::InstalledPluginPackage,
    manifest_record: &'a Value,
) -> Result<&'a str, String> {
    if package.plugin_ref != request.plugin_ref {
        return Err("plugin_package_identity_mismatch".to_string());
    }
    let manifest_digest = required_string(manifest_record, "manifestDigest")?;
    if manifest_digest != package.manifest_sha256
        || manifest_digest != request.manifest_fingerprint
        || manifest_record["package"]["packageSha256"] != package.package_sha256
    {
        return Err("plugin_manifest_binding_mismatch".to_string());
    }
    Ok(manifest_digest)
}

fn validate_instance(request: &PluginOperationRequest, instance: &Value) -> Result<(), String> {
    if instance
        .get("removedAtMs")
        .is_some_and(|value| !value.is_null())
    {
        return Err("plugin_instance_removed".to_string());
    }
    if !instance["enabled"].as_bool().unwrap_or(false) {
        return Err("plugin_instance_disabled".to_string());
    }
    if instance["instanceRef"] != request.plugin_instance_ref
        || instance["pluginRef"] != request.plugin_ref
    {
        return Err("plugin_instance_binding_mismatch".to_string());
    }
    if request.operation_id.starts_with("broker.") {
        let instance_mode = instance["configuration"]["mode"]
            .as_str()
            .or_else(|| instance["accountMode"].as_str());
        if instance_mode.is_some_and(|mode| mode != request.mode) {
            return Err("plugin_instance_mode_mismatch".to_string());
        }
    }
    Ok(())
}

fn declared_operation<'a>(
    manifest: &'a Value,
    request: &PluginOperationRequest,
) -> Result<&'a Value, String> {
    let capability_declared = manifest["capabilities"]
        .as_array()
        .is_some_and(|capabilities| {
            capabilities
                .iter()
                .any(|capability| capability["id"] == request.capability)
        });
    if !capability_declared {
        return Err("plugin_capability_not_declared".to_string());
    }
    let operation = manifest["operations"]
        .as_array()
        .and_then(|operations| {
            operations
                .iter()
                .find(|operation| operation["id"] == request.operation_id)
        })
        .ok_or_else(|| "plugin_operation_not_declared".to_string())?;
    if operation["capability"] != request.capability {
        return Err("plugin_operation_capability_mismatch".to_string());
    }
    if operation["traits"]["modes"]
        .as_array()
        .is_some_and(|modes| !modes.is_empty() && !modes.iter().any(|mode| mode == &request.mode))
    {
        return Err("plugin_operation_mode_unsupported".to_string());
    }
    Ok(operation)
}

fn operation_timeout_ms(manifest: &Value, requested_ms: u64) -> Result<u64, String> {
    let manifest_ms = manifest["runtime"]["timeoutSeconds"]
        .as_u64()
        .unwrap_or(15)
        .checked_mul(1_000)
        .ok_or_else(|| "plugin_timeout_invalid".to_string())?;
    let timeout = requested_ms.min(manifest_ms);
    if timeout == 0 {
        Err("plugin_timeout_invalid".to_string())
    } else {
        Ok(timeout)
    }
}

fn build_wire_request(
    request: &PluginOperationRequest,
    context: &SideEffectContext,
    instance: &Value,
    package_digest: &str,
    manifest_digest: &str,
    timeout_ms: u64,
    credential_values: std::collections::BTreeMap<String, String>,
) -> Result<PluginRequest, String> {
    let mut payload = request.input.as_object().cloned().unwrap_or_else(Map::new);
    payload.insert(
        "config".to_string(),
        instance
            .get("configuration")
            .cloned()
            .unwrap_or_else(|| json!({})),
    );
    let payload = match request.operation_id.as_str() {
        operation if operation.starts_with("broker.") => {
            RequestPayload::BrokerOrder(Value::Object(payload))
        }
        operation if operation.contains("bars") => {
            RequestPayload::HistoricalData(Value::Object(payload))
        }
        "account.health" | "account.discovery" => RequestPayload::Health(Value::Object(payload)),
        "plugin.capability_matrix" => RequestPayload::CapabilityMatrix(Value::Object(payload)),
        _ => RequestPayload::Operation(Value::Object(payload)),
    };
    Ok(PluginRequest {
        schema_version: WIRE_CONTRACT_VERSION.to_string(),
        host_contract_version: WIRE_CONTRACT_VERSION.to_string(),
        sdk_version: SDK_VERSION.to_string(),
        request_id: context.idempotency_key.as_str().to_string(),
        metadata: RequestMetadata {
            operation_id: request.operation_id.clone(),
            plugin_id: request.plugin_ref.clone(),
            plugin_instance_id: request.plugin_instance_ref.clone(),
            package_digest: package_digest.to_string(),
            manifest_digest: manifest_digest.to_string(),
            capability_binding_id: request.capability_graph_revision_id.clone(),
            capability_fingerprint: request.capability_graph_fingerprint.clone(),
        },
        context: RequestContext {
            authority_id: context.authority.actor.clone(),
            purpose: request.purpose.clone(),
            mode: request.mode.clone(),
            account_id: request
                .account_ref
                .clone()
                .or_else(|| instance["accountRef"].as_str().map(str::to_string)),
            fencing_token: request
                .fencing_token
                .map(|token| token.to_string())
                .unwrap_or_else(|| "none".to_string()),
            timeout_ms,
            idempotency_key: context.idempotency_key.as_str().to_string(),
            strategy_id: Some(request.strategy_id.clone()),
            activation_id: Some(request.activation_id.clone()),
            attempt_id: Some(request.attempt_id.clone()),
            tick_id: Some(request.evaluation_tick_id.clone()),
            evidence_references: request.evidence_refs.clone(),
        },
        payload,
        ephemeral_credential_grant: (!credential_values.is_empty())
            .then(|| EphemeralCredentialGrant::new(credential_values)),
    })
}

fn allowed_network_domains(manifest: &Value, instance: &Value) -> Result<Vec<String>, String> {
    let outbound_declared = manifest["permissions"]
        .as_array()
        .is_some_and(|permissions| {
            permissions.iter().any(|permission| {
                permission.as_str() == Some("network.outbound")
                    || permission["id"].as_str() == Some("network.outbound")
            })
        });
    if !outbound_declared {
        return Ok(Vec::new());
    }

    let url_fields = manifest["configuration"]["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|field| field["inputType"] == "url" && field["storageClass"] == "configuration")
        .filter_map(|field| field["id"].as_str());
    let configuration = instance["configuration"]
        .as_object()
        .ok_or_else(|| "plugin_network_configuration_invalid".to_string())?;
    let mut domains = BTreeSet::new();
    for field in url_fields {
        let Some(raw_url) = configuration.get(field).and_then(Value::as_str) else {
            continue;
        };
        let url = url::Url::parse(raw_url)
            .map_err(|_| "plugin_network_configuration_invalid".to_string())?;
        let host = url
            .host_str()
            .ok_or_else(|| "plugin_network_configuration_invalid".to_string())?
            .trim_matches(['[', ']'])
            .to_ascii_lowercase();
        let loopback = matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1");
        if (!loopback && url.scheme() != "https")
            || (loopback && !matches!(url.scheme(), "http" | "https"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("plugin_network_configuration_invalid".to_string());
        }
        domains.insert(host);
    }
    Ok(domains.into_iter().collect())
}

fn execute(
    sandbox: &dyn PluginProcessSandboxPort,
    executable: &std::path::Path,
    install_root: &std::path::Path,
    allowed_domains: Vec<String>,
    request: &PluginRequest,
    timeout_ms: u64,
) -> Result<PluginResponse, String> {
    let mut child = sandbox.spawn(&PluginSandboxRequest {
        executable: executable.to_path_buf(),
        install_root: install_root.to_path_buf(),
        allowed_domains,
    })?;
    let mut stdin = child
        .take_stdin()
        .ok_or_else(|| "plugin_process_stdin_unavailable".to_string())?;
    write_request(&mut stdin, request, DEFAULT_MAX_ENVELOPE_BYTES)
        .map_err(|_| "plugin_request_write_failed".to_string())?;
    drop(stdin);

    let stdout = child
        .take_stdout()
        .ok_or_else(|| "plugin_process_stdout_unavailable".to_string())?;
    let reader = thread::spawn(move || {
        read_response(BufReader::new(stdout), DEFAULT_MAX_ENVELOPE_BYTES)
            .map_err(|_| "plugin_response_invalid".to_string())
    });
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let status = loop {
        match child
            .try_wait()
            .map_err(|_| "plugin_process_wait_failed".to_string())?
        {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err("plugin_process_timeout".to_string());
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
    };
    let response = reader.join();
    if !status.success() {
        // Exit status is safe operational evidence; never forward plugin stderr,
        // which can contain provider payloads or credentials.
        return Err(match status.code() {
            Some(code) => format!("plugin_process_failed:exit_code={code}"),
            None => "plugin_process_failed:terminated".to_string(),
        });
    }
    response.map_err(|_| "plugin_response_invalid".to_string())?
}

fn response_from_wire(
    request: &PluginOperationRequest,
    wire_request: &PluginRequest,
    response: PluginResponse,
    operation: &Value,
    now_ms: i64,
) -> Result<PluginOperationResponse, String> {
    if !operation["traits"]["outputSchemaRefs"]
        .as_array()
        .is_some_and(|schemas| {
            schemas
                .iter()
                .any(|schema| schema.as_str() == Some(response.response_schema.as_str()))
        })
    {
        return Err("plugin_response_schema_undeclared".to_string());
    }
    if response.status == ResponseStatus::Failed {
        return Err(format!(
            "plugin_operation_failed:{}",
            response.provider_outcome.code
        ));
    }
    let reconciliation_required = response.status == ResponseStatus::ReconciliationRequired
        || response.reconciliation == Reconciliation::Required;
    let observed_at_ms = now_ms;
    let source_event_id = format!(
        "plugin-event-{}",
        short_hash(
            format!(
                "{}:{}:{}",
                request.evaluation_tick_id,
                response.request_id,
                wire_request.metadata.package_digest
            )
            .as_bytes()
        )
    );
    let content_hash = short_hash(
        serde_json::to_vec(&json!({
            "schemaRef": response.response_schema,
            "payload": response.payload,
            "observedAtMs": observed_at_ms,
            "sourceEventId": source_event_id,
            "providerOutcome": response.provider_outcome,
            "reconciliation": response.reconciliation,
            "pluginId": wire_request.metadata.plugin_id,
            "pluginInstanceId": wire_request.metadata.plugin_instance_id,
            "packageDigest": wire_request.metadata.package_digest,
            "manifestDigest": wire_request.metadata.manifest_digest,
            "capabilityBindingId": wire_request.metadata.capability_binding_id,
            "capabilityFingerprint": wire_request.metadata.capability_fingerprint,
        }))
        .map_err(|_| "plugin_operation_response_invalid".to_string())?
        .as_slice(),
    );
    Ok(PluginOperationResponse {
        correlation_id: request.correlation_id.clone(),
        schema_ref: response.response_schema,
        payload: response.payload,
        observed_at_ms,
        source_event_id,
        content_hash,
        freshness_state: if reconciliation_required {
            "unknown".to_string()
        } else {
            "fresh".to_string()
        },
        evidence_refs: response.evidence_references,
        deterministic: operation["traits"]["deterministic"]
            .as_bool()
            .unwrap_or(false),
        replayable: operation["traits"]["replayable"].as_bool().unwrap_or(false),
        provider_outcome_id: response
            .provider_outcome
            .provider_reference
            .or(response.provider_outcome.provider_request_id),
        reconciliation_required,
    })
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("plugin_{field}_missing"))
}

fn short_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))[..24].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        FailureMode, InstalledPluginPackage, PluginOperationRequest, PortDescriptor, PortKind,
        SandboxedPluginProcess, VersionedPort,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    use tradeassembly_plugin_sdk::{PluginRequest, PluginResponse};

    fn operation_request() -> PluginOperationRequest {
        PluginOperationRequest {
            correlation_id: "correlation-1".to_string(),
            plugin_instance_ref: "alpaca-paper".to_string(),
            plugin_ref: "tradeassembly.alpaca".to_string(),
            manifest_fingerprint: "manifest-digest".to_string(),
            operation_id: "account.health".to_string(),
            capability: "account.health".to_string(),
            capability_graph_revision_id: "binding-direct".to_string(),
            capability_graph_fingerprint: "binding-fingerprint".to_string(),
            strategy_id: String::new(),
            strategy_version_id: String::new(),
            strategy_spec_hash: String::new(),
            activation_id: String::new(),
            attempt_id: String::new(),
            evaluation_tick_id: String::new(),
            mode: "paper".to_string(),
            purpose: "account_health".to_string(),
            account_ref: Some("brokerage-account:example".to_string()),
            timeout_ms: 1_000,
            fencing_token: None,
            input: json!({}),
            evidence_refs: Vec::new(),
        }
    }

    fn wire_request() -> PluginRequest {
        serde_json::from_value(json!({
            "schemaVersion": "1",
            "hostContractVersion": "1",
            "sdkVersion": "0.1.0",
            "requestId": "request-1",
            "metadata": {
                "operationId": "account.health",
                "pluginId": "tradeassembly.alpaca",
                "pluginInstanceId": "alpaca-paper",
                "packageDigest": "package-digest",
                "manifestDigest": "manifest-digest",
                "capabilityBindingId": "binding-direct",
                "capabilityFingerprint": "binding-fingerprint"
            },
            "context": {
                "authorityId": "local-user",
                "purpose": "account_health",
                "mode": "paper",
                "accountId": null,
                "fencingToken": "none",
                "timeoutMs": 1000,
                "idempotencyKey": "request-1",
                "strategyId": null,
                "activationId": null,
                "attemptId": null,
                "tickId": null
            },
            "payload": {"kind": "health", "payload": {}},
            "ephemeralCredentialGrant": null
        }))
        .expect("wire request")
    }

    fn wire_response(payload: Value) -> PluginResponse {
        serde_json::from_value(json!({
            "schemaVersion": "1",
            "requestId": "request-1",
            "status": "succeeded",
            "responseSchema": "schema://tradeassembly.alpaca/account-response@1",
            "payload": payload,
            "providerOutcome": {"code": "ok"},
            "reconciliation": "not_required"
        }))
        .expect("wire response")
    }

    fn operation() -> Value {
        json!({
            "effect": "read",
            "traits": {
                "outputSchemaRefs": ["schema://tradeassembly.alpaca/account-response@1"]
            }
        })
    }

    #[test]
    fn outbound_domains_require_permission_and_declared_url_configuration() {
        let manifest = json!({
            "permissions": [{"id": "network.outbound"}],
            "configuration": {
                "fields": [
                    {
                        "id": "base_url",
                        "inputType": "url",
                        "storageClass": "configuration"
                    },
                    {
                        "id": "note",
                        "inputType": "text",
                        "storageClass": "configuration"
                    }
                ]
            }
        });
        let instance = json!({
            "configuration": {
                "base_url": "https://DATA.ALPACA.MARKETS/v2",
                "note": "https://attacker.invalid"
            }
        });
        assert_eq!(
            allowed_network_domains(&manifest, &instance).expect("declared domain"),
            vec!["data.alpaca.markets".to_string()]
        );

        let mut no_permission = manifest.clone();
        no_permission["permissions"] = json!([]);
        assert_eq!(
            allowed_network_domains(&no_permission, &instance).expect("default deny"),
            Vec::<String>::new()
        );

        let credentials_in_url = json!({
            "configuration": {"base_url": "https://user:secret@example.com"}
        });
        assert_eq!(
            allowed_network_domains(&manifest, &credentials_in_url),
            Err("plugin_network_configuration_invalid".to_string())
        );
    }

    struct NativeTestSandbox;

    impl VersionedPort for NativeTestSandbox {
        fn descriptors(&self) -> Vec<PortDescriptor> {
            let mut descriptor = PortDescriptor::new(PortKind::Plugins, "test.plugin-sandbox");
            descriptor.failure_mode = FailureMode::FailClosed;
            vec![descriptor]
        }
    }

    impl PluginProcessSandboxPort for NativeTestSandbox {
        fn spawn(&self, request: &PluginSandboxRequest) -> Result<SandboxedPluginProcess, String> {
            let child = Command::new(&request.executable)
                .current_dir(&request.install_root)
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| "test_plugin_process_spawn_failed".to_string())?;
            Ok(SandboxedPluginProcess::new(child, None))
        }
    }

    #[test]
    fn disabled_and_removed_instances_are_rejected() {
        let request = operation_request();
        let disabled = json!({
            "instanceRef": "alpaca-paper",
            "pluginRef": "tradeassembly.alpaca",
            "enabled": false,
            "configuration": {"mode": "paper"}
        });
        assert_eq!(
            validate_instance(&request, &disabled),
            Err("plugin_instance_disabled".to_string())
        );

        let removed = json!({
            "instanceRef": "alpaca-paper",
            "pluginRef": "tradeassembly.alpaca",
            "enabled": true,
            "removedAtMs": 42,
            "configuration": {"mode": "paper"}
        });
        assert_eq!(
            validate_instance(&request, &removed),
            Err("plugin_instance_removed".to_string())
        );
    }

    #[test]
    fn stale_package_manifest_binding_is_rejected() {
        let request = operation_request();
        let package = InstalledPluginPackage {
            plugin_ref: "tradeassembly.alpaca".to_string(),
            version: "0.1.0".to_string(),
            package_sha256: "package-digest".to_string(),
            manifest_sha256: "stale-manifest-digest".to_string(),
            manifest: json!({}),
            descriptor: json!({}),
            install_root: Default::default(),
            executable_path: Default::default(),
            host_contract: "1".to_string(),
            sdk_version: "0.1.0".to_string(),
        };
        let manifest_record = json!({
            "manifestDigest": "manifest-digest",
            "package": {"packageSha256": "package-digest"}
        });
        assert_eq!(
            validate_package_binding(&request, &package, &manifest_record),
            Err("plugin_manifest_binding_mismatch".to_string())
        );
    }

    #[test]
    fn response_secrets_are_rejected_by_value_and_field_shape() {
        let request = wire_request();
        let response = wire_response(json!({"account": {"note": "secret-value"}}));
        let credentials = BTreeMap::from([("api_secret".to_string(), "secret-value".to_string())]);
        assert_eq!(
            crate::adapters::plugin_output_policy::validate_plugin_response_policy(
                &request,
                &response,
                &operation(),
                &credentials,
            ),
            Err("plugin_response_secret_leak".to_string())
        );

        let response = wire_response(json!({"account": {"api_key": "redacted"}}));
        assert_eq!(
            crate::adapters::plugin_output_policy::validate_plugin_response_policy(
                &request,
                &response,
                &operation(),
                &BTreeMap::new(),
            ),
            Err("plugin_response_secret_field_forbidden".to_string())
        );
    }

    #[test]
    fn response_provenance_is_derived_from_host_inputs() {
        let request = operation_request();
        let wire_request = wire_request();
        let response = wire_response(json!({
            "observedAtMs": -1,
            "sourceEventId": "plugin-controlled",
            "account": {"status": "connected"}
        }));
        let projected = response_from_wire(&request, &wire_request, response, &operation(), 42)
            .expect("host projection");
        assert_eq!(projected.observed_at_ms, 42);
        assert_ne!(projected.source_event_id, "plugin-controlled");
        assert!(projected.source_event_id.starts_with("plugin-event-"));
    }

    #[test]
    fn response_payload_must_match_digested_installed_schema() {
        let directory = tempfile::tempdir().expect("schema package");
        fs::create_dir_all(directory.path().join("schemas")).expect("schema directory");
        let schema = serde_json::to_vec(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "required": ["account"],
            "additionalProperties": false,
            "properties": {
                "account": {"type": "string"}
            }
        }))
        .expect("schema bytes");
        fs::write(directory.path().join("schemas/account.json"), &schema).expect("write schema");
        let package = InstalledPluginPackage {
            plugin_ref: "tradeassembly.alpaca".to_string(),
            version: "0.1.0".to_string(),
            package_sha256: "package-digest".to_string(),
            manifest_sha256: "manifest-digest".to_string(),
            manifest: json!({}),
            descriptor: json!({
                "packageContractVersion": "1",
                "plugin": {"id": "tradeassembly.alpaca", "version": "0.1.0"},
                "manifest": {"path": "manifest.yaml", "sha256": "a".repeat(64)},
                "targets": [{
                    "target": tradeassembly_plugin_sdk::host_target(),
                    "binary": {"path": "bin/plugin", "sha256": "b".repeat(64)}
                }],
                "responseSchemas": [{
                    "schemaRef": "schema://tradeassembly.alpaca/account-response@1",
                    "document": {
                        "path": "schemas/account.json",
                        "sha256": format!("{:x}", Sha256::digest(&schema))
                    }
                }],
                "compatibility": {
                    "hostContract": {"minimum": "1", "maximum": "1"},
                    "sdkVersion": "0.1.0"
                }
            }),
            install_root: directory.path().to_path_buf(),
            executable_path: directory.path().join("bin/plugin"),
            host_contract: "1".to_string(),
            sdk_version: "0.1.0".to_string(),
        };
        assert!(validate_response_schema_document(
            &package,
            &wire_response(json!({"account": "connected"}))
        )
        .is_ok());
        assert_eq!(
            validate_response_schema_document(&package, &wire_response(json!({"account": 42}))),
            Err("plugin_response_payload_schema_mismatch".to_string())
        );
        fs::write(
            directory.path().join("schemas/account.json"),
            br#"{"type":"object"}"#,
        )
        .expect("tamper schema");
        assert_eq!(
            validate_response_schema_document(
                &package,
                &wire_response(json!({"account": "connected"}))
            ),
            Err("plugin_response_schema_digest_mismatch".to_string())
        );
    }

    #[cfg(unix)]
    fn executable_script(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().expect("temporary plugin");
        let path = directory.path().join("plugin");
        fs::write(&path, contents).expect("write plugin script");
        let mut permissions = fs::metadata(&path).expect("plugin metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("make plugin executable");
        (directory, path)
    }

    #[cfg(unix)]
    #[test]
    fn timed_out_plugin_process_is_terminated() {
        let (directory, executable) =
            executable_script("#!/bin/sh\n/bin/cat >/dev/null\nexec /bin/sleep 2\n");
        let started = Instant::now();
        assert_eq!(
            execute(
                &NativeTestSandbox,
                &executable,
                directory.path(),
                Vec::new(),
                &wire_request(),
                25,
            )
            .unwrap_err(),
            "plugin_process_timeout"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn failed_process_is_not_misreported_as_malformed_response() {
        for output in ["", "printf 'private-diagnostic-do-not-expose\\n'\n"] {
            let script = format!("#!/bin/sh\nIFS= read -r request\n{output}exit 7\n");
            let (directory, executable) = executable_script(&script);
            assert_eq!(
                execute(
                    &NativeTestSandbox,
                    &executable,
                    directory.path(),
                    Vec::new(),
                    &wire_request(),
                    1_000,
                )
                .unwrap_err(),
                "plugin_process_failed:exit_code=7"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn malformed_and_multiple_plugin_responses_are_rejected() {
        let (directory, executable) =
            executable_script("#!/bin/sh\nIFS= read -r request\nprintf 'not-json\\n'\n");
        assert_eq!(
            execute(
                &NativeTestSandbox,
                &executable,
                directory.path(),
                Vec::new(),
                &wire_request(),
                1_000,
            )
            .unwrap_err(),
            "plugin_response_invalid"
        );

        let encoded =
            serde_json::to_string(&wire_response(json!({"ok": true}))).expect("encode response");
        let script = format!(
            "#!/bin/sh\nIFS= read -r request\nprintf '%s\\n%s\\n' '{encoded}' '{encoded}'\n"
        );
        let (directory, executable) = executable_script(&script);
        assert_eq!(
            execute(
                &NativeTestSandbox,
                &executable,
                directory.path(),
                Vec::new(),
                &wire_request(),
                1_000,
            )
            .unwrap_err(),
            "plugin_response_invalid"
        );
    }
}
