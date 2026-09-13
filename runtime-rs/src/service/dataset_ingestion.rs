// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use super::{capability_graph, providers, strategy, TradeAssemblyService};
use crate::historical_data::{
    AssetClass, DatasetIngestion, DatasetIngestionState, DatasetNormalizationPolicy,
    DatasetQualityFinding, DatasetQualityPolicy, DatasetQualitySeverity, DatasetSnapshot,
    DatasetSnapshotContent, DatasetSourceClass, DatasetSourceManifest, DatasetTimeSlice,
    HistoricalDataKind, HistoricalObservation, HistoricalObservationData, QualityDisposition,
    DATASET_SNAPSHOT_SCHEMA,
};
use crate::ports::{
    AuthorityContext, CapabilityBinding, ComparePutOutcome, EvidenceRecord, HistoricalDataRequest,
    IdempotencyKey, ImmutablePutOutcome, SideEffectContext,
};
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const INGESTIONS_NS: &str = "dataset_ingestions_v1";
const IDEMPOTENCY_NS: &str = "dataset_ingestion_idempotency_v1";
const REQUIREMENT_ID: &str = "dataset.ingestion.source";
const MAX_TOTAL_ROWS: u64 = 250_000;
const MAX_PAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_PAGES: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DatasetIngestionCommand {
    authority_actor: String,
    strategy_id: String,
    strategy_version_id: String,
    plugin_instance_ref: String,
    plugin_ref: String,
    operation_id: String,
    asset_class: AssetClass,
    instruments: Vec<String>,
    data_kind: HistoricalDataKind,
    granularity: String,
    time_slice: DatasetTimeSlice,
    calendar: String,
    timezone: String,
    normalization_policy: DatasetNormalizationPolicy,
    quality_policy: DatasetQualityPolicy,
    max_rows: u64,
    supplied_capability_graph_revision_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DatasetIngestionRecord {
    aggregate: DatasetIngestion,
    command: DatasetIngestionCommand,
    source: Option<ResolvedDatasetSource>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResolvedDatasetSource {
    plugin_instance_ref: String,
    plugin_ref: String,
    operation_id: String,
    plugin_manifest_fingerprint: String,
    capability_graph_revision_id: String,
    capability_graph_fingerprint: String,
    credential_handle: Option<String>,
}

pub(crate) fn create(service: &TradeAssemblyService, body: Value) -> Value {
    match create_result(service, &body) {
        Ok(record) => response(service, &record),
        Err(code) => json!({
            "status": failure_state(&code),
            "error": {"code": code},
            "noAdvice": true,
        }),
    }
}

pub(crate) fn get(service: &TradeAssemblyService, ingestion_id: &str) -> Value {
    if service
        .require_object("dataset_ingestion", ingestion_id)
        .is_err()
    {
        return not_found();
    }
    read_record(service, ingestion_id)
        .map(|record| response(service, &record))
        .unwrap_or_else(|code| json!({"status": "not_found", "error": {"code": code}}))
}

pub(crate) fn list(service: &TradeAssemblyService) -> Vec<Value> {
    let values = service
        .runtime()
        .storage
        .list_json(INGESTIONS_NS)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, value)| serde_json::from_value::<DatasetIngestionRecord>(value).ok())
        .map(|record| response(service, &record))
        .collect::<Vec<_>>();
    service.filter_visible_values("dataset_ingestion", values, &["ingestionId"])
}

pub(crate) fn verify(service: &TradeAssemblyService, ingestion_id: &str) -> Value {
    if service
        .require_object("dataset_ingestion", ingestion_id)
        .is_err()
    {
        return not_found();
    }
    match read_record(service, ingestion_id) {
        Ok(record) => {
            let Some(snapshot_id) = record.aggregate.snapshot_id.as_deref() else {
                return json!({
                    "status": "not_verifiable",
                    "ingestionId": record.aggregate.ingestion_id,
                    "snapshotId": Value::Null,
                    "verified": false,
                    "error": {"code": "dataset_snapshot_not_available"},
                    "noAdvice": true,
                });
            };
            let mut payload = response(service, &record);
            if payload.get("error").is_some() {
                if let Some(object) = payload.as_object_mut() {
                    object.insert("verified".to_string(), Value::Bool(false));
                }
                payload
            } else {
                json!({
                    "status": "verified",
                    "ingestionId": record.aggregate.ingestion_id,
                    "snapshotId": snapshot_id,
                    "verified": true,
                    "ingestion": payload,
                    "noAdvice": true,
                })
            }
        }
        Err(code) => json!({
            "status": "not_found",
            "verified": false,
            "error": {"code": code},
            "noAdvice": true,
        }),
    }
}

pub(crate) fn cancel(service: &TradeAssemblyService, ingestion_id: &str, body: &Value) -> Value {
    if service
        .require_object("dataset_ingestion", ingestion_id)
        .is_err()
    {
        return not_found();
    }
    let mut record = match read_record(service, ingestion_id) {
        Ok(record) => record,
        Err(code) => return json!({"status": "not_found", "error": {"code": code}}),
    };
    if record.aggregate.state.is_terminal() {
        return json!({
            "status": "conflict",
            "error": {"code": "dataset_ingestion_canceled"},
            "ingestion": record.aggregate,
        });
    }
    let context = context(body, &record.aggregate.idempotency_key);
    if context.authority.actor != record.command.authority_actor {
        return json!({
            "status": "forbidden",
            "error": {"code": "dataset_authority_mismatch"},
            "noAdvice": true,
        });
    }
    if let Err(code) = transition(
        service,
        &mut record,
        DatasetIngestionState::Canceled,
        &context,
        Some("dataset_ingestion_canceled"),
    ) {
        return json!({"status": "failed", "error": {"code": code}});
    }
    response(service, &record)
}

fn create_result(
    service: &TradeAssemblyService,
    body: &Value,
) -> Result<DatasetIngestionRecord, String> {
    let command = parse_command(service, body)?;
    service
        .require_object("strategy", &command.strategy_id)
        .map_err(|_| "dataset_ingestion_not_found".to_string())?;
    let request_hash = hash_canonical(&command)?;
    let idempotency_key = body
        .get("idempotencyKey")
        .or_else(|| body.get("idempotency_key"))
        .or_else(|| body.get("commandId"))
        .or_else(|| body.get("command_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("dataset.ingest:{request_hash}"));
    let idempotency_ref = hash_string(&idempotency_key);
    let context = context(body, &idempotency_key);
    let ingestion_id = format!("ingestion_{}", &hash_string(&idempotency_key)[..24]);
    service
        .bind_inherited_object(
            "dataset_ingestion",
            &ingestion_id,
            "strategy",
            &command.strategy_id,
        )
        .map_err(|_| "dataset_ingestion_not_found".to_string())?;

    if let Some(existing) = service
        .runtime()
        .storage
        .get_json(IDEMPOTENCY_NS, &idempotency_ref)
        .map_err(|_| "dataset_ingestion_persistence_failed".to_string())?
    {
        if existing["requestHash"].as_str() != Some(request_hash.as_str()) {
            return Err("dataset_ingestion_conflict".to_string());
        }
        let ingestion_id = existing["ingestionId"]
            .as_str()
            .ok_or_else(|| "dataset_ingestion_integrity_failed".to_string())?;
        if ingestion_id != format!("ingestion_{}", &hash_string(&idempotency_key)[..24]) {
            return Err("dataset_ingestion_integrity_failed".to_string());
        }
        let mut record = match read_record(service, ingestion_id) {
            Ok(record) => record,
            Err(code) if code == "dataset_ingestion_not_found" => {
                let record = initial_record(
                    ingestion_id.to_string(),
                    idempotency_key.clone(),
                    request_hash.clone(),
                    command.clone(),
                    service.runtime().clock.now_ms(),
                );
                persist_record(service, &record, &context)?;
                record_evidence(service, &record, &context)?;
                record
            }
            Err(code) => return Err(code),
        };
        if record.aggregate.state.is_terminal() {
            return Ok(record);
        }
        let previous = record.clone();
        record.aggregate.attempt = record.aggregate.attempt.saturating_add(1);
        record.aggregate.fencing_token = record.aggregate.fencing_token.saturating_add(1);
        record.aggregate.updated_at_ms = service.runtime().clock.now_ms();
        replace_record(service, &previous, &record, &context)?;
        return run(service, record, &context);
    }

    let now = service.runtime().clock.now_ms();
    let record = initial_record(
        ingestion_id.clone(),
        idempotency_key,
        request_hash.clone(),
        command,
        now,
    );
    let outcome = service
        .runtime()
        .storage
        .put_json_if_absent(
            IDEMPOTENCY_NS,
            &idempotency_ref,
            json!({"requestHash": request_hash, "ingestionId": ingestion_id}),
            &context,
        )
        .map_err(|_| "dataset_ingestion_conflict".to_string())?;
    if outcome == ImmutablePutOutcome::AlreadyPresent {
        return create_result(service, body);
    }
    persist_record(service, &record, &context)?;
    record_evidence(service, &record, &context)?;
    run(service, record, &context)
}

fn initial_record(
    ingestion_id: String,
    idempotency_key: String,
    request_hash: String,
    command: DatasetIngestionCommand,
    now: i64,
) -> DatasetIngestionRecord {
    DatasetIngestionRecord {
        aggregate: DatasetIngestion {
            ingestion_id,
            idempotency_key,
            request_hash,
            strategy_id: command.strategy_id.clone(),
            strategy_version_id: Some(command.strategy_version_id.clone()),
            state: DatasetIngestionState::Requested,
            attempt: 1,
            fencing_token: 1,
            requested_at_ms: now,
            updated_at_ms: now,
            snapshot_id: None,
            blockers: Vec::new(),
            diagnostics: Vec::new(),
        },
        command,
        source: None,
    }
}

fn run(
    service: &TradeAssemblyService,
    mut record: DatasetIngestionRecord,
    context: &SideEffectContext,
) -> Result<DatasetIngestionRecord, String> {
    if record.aggregate.state == DatasetIngestionState::Canceled {
        return Err("dataset_ingestion_canceled".to_string());
    }
    transition(
        service,
        &mut record,
        DatasetIngestionState::ResolvingSource,
        context,
        None,
    )?;
    let source = match resolve_source(
        service,
        &record.command,
        &record.aggregate.request_hash,
        &record.aggregate.idempotency_key,
    ) {
        Ok(source) => source,
        Err(code) => {
            transition(
                service,
                &mut record,
                DatasetIngestionState::Blocked,
                context,
                Some(&code),
            )?;
            return Ok(record);
        }
    };
    record.source = Some(source.clone());
    transition(
        service,
        &mut record,
        DatasetIngestionState::Ingesting,
        context,
        None,
    )?;
    let acquired = match acquire(service, &record, &source, context) {
        Ok(acquired) => acquired,
        Err(code) => {
            transition(
                service,
                &mut record,
                failure_state_enum(&code),
                context,
                Some(&code),
            )?;
            return Ok(record);
        }
    };
    transition(
        service,
        &mut record,
        DatasetIngestionState::Validating,
        context,
        None,
    )?;
    let (observations, diagnostics) = match normalize_and_validate(&record.command, acquired.rows) {
        Ok(value) => value,
        Err(code) => {
            transition(
                service,
                &mut record,
                DatasetIngestionState::Failed,
                context,
                Some(&code),
            )?;
            return Ok(record);
        }
    };
    record.aggregate.diagnostics = diagnostics.clone();
    let snapshot = match DatasetSnapshot::build(
        DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
            strategy_id: record.command.strategy_id.clone(),
            strategy_version_id: Some(record.command.strategy_version_id.clone()),
            source: DatasetSourceManifest {
                source_class: acquired.source_class,
                plugin_instance_ref: source.plugin_instance_ref.clone(),
                plugin_ref: source.plugin_ref.clone(),
                operation_id: source.operation_id.clone(),
                plugin_manifest_fingerprint: source.plugin_manifest_fingerprint.clone(),
                capability_graph_revision_id: source.capability_graph_revision_id.clone(),
                capability_graph_fingerprint: source.capability_graph_fingerprint.clone(),
                source_request_hash: record.aggregate.request_hash.clone(),
                upstream_refs: acquired.upstream_refs,
            },
            asset_classes: vec![record.command.asset_class.clone()],
            instruments: record.command.instruments.clone(),
            data_kind: record.command.data_kind.clone(),
            granularity: record.command.granularity.clone(),
            time_slice: record.command.time_slice.clone(),
            calendar: record.command.calendar.clone(),
            timezone: record.command.timezone.clone(),
            normalization_policy: record.command.normalization_policy.clone(),
            quality_policy: record.command.quality_policy.clone(),
            quality_findings: diagnostics,
            observations,
        },
        service.runtime().clock.now_ms(),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let code = stable_snapshot_error(error);
            transition(
                service,
                &mut record,
                DatasetIngestionState::Failed,
                context,
                Some(&code),
            )?;
            return Ok(record);
        }
    };
    ensure_record_current(service, &record)?;
    service
        .bind_inherited_object(
            "dataset_snapshot",
            &snapshot.dataset_id,
            "dataset_ingestion",
            &record.aggregate.ingestion_id,
        )
        .map_err(|_| "dataset_ingestion_not_found".to_string())?;
    if let Err(error) = service.runtime().dataset_snapshots.put(&snapshot, context) {
        let code = stable_snapshot_error(error);
        transition(
            service,
            &mut record,
            DatasetIngestionState::Failed,
            context,
            Some(&code),
        )?;
        return Ok(record);
    }
    record.aggregate.snapshot_id = Some(snapshot.dataset_id);
    transition(
        service,
        &mut record,
        DatasetIngestionState::Completed,
        context,
        None,
    )?;
    Ok(record)
}

struct AcquiredRows {
    source_class: DatasetSourceClass,
    rows: Vec<HistoricalObservation>,
    upstream_refs: BTreeMap<String, String>,
}

fn acquire(
    service: &TradeAssemblyService,
    record: &DatasetIngestionRecord,
    source: &ResolvedDatasetSource,
    context: &SideEffectContext,
) -> Result<AcquiredRows, String> {
    let command = &record.command;
    let mut page_token = None;
    let mut seen_tokens = BTreeSet::new();
    let mut rows = Vec::new();
    let mut source_class = None;
    let mut upstream_refs = BTreeMap::new();
    for page_index in 0..MAX_PAGES {
        ensure_record_current(service, record)?;
        let remaining = command.max_rows.saturating_sub(rows.len() as u64);
        if remaining == 0 {
            return Err("dataset_response_too_large".to_string());
        }
        let request = HistoricalDataRequest {
            plugin_instance_ref: source.plugin_instance_ref.clone(),
            plugin_ref: source.plugin_ref.clone(),
            operation_id: source.operation_id.clone(),
            plugin_manifest_fingerprint: source.plugin_manifest_fingerprint.clone(),
            capability_graph_revision_id: source.capability_graph_revision_id.clone(),
            capability_graph_fingerprint: source.capability_graph_fingerprint.clone(),
            asset_class: command.asset_class.clone(),
            instruments: command.instruments.clone(),
            data_kind: command.data_kind.clone(),
            granularity: command.granularity.clone(),
            time_slice: command.time_slice.clone(),
            calendar: command.calendar.clone(),
            timezone: command.timezone.clone(),
            normalization_policy: command.normalization_policy.clone(),
            quality_policy: command.quality_policy.clone(),
            credential_handle: source.credential_handle.clone(),
            page_token: page_token.clone(),
            max_rows: remaining.min(10_000),
        };
        let page = service
            .runtime()
            .historical_data
            .acquire_page(&request, context)
            .map_err(stable_source_error)?;
        ensure_record_current(service, record)?;
        let page_bytes =
            serde_json::to_vec(&page).map_err(|_| "dataset_schema_unsupported".to_string())?;
        if page_bytes.len() > MAX_PAGE_BYTES {
            return Err("dataset_response_too_large".to_string());
        }
        if let Some(expected) = &source_class {
            if expected != &page.source_class {
                return Err("dataset_source_binding_required".to_string());
            }
        } else {
            source_class = Some(page.source_class.clone());
        }
        validate_source_class(source, &page.source_class)?;
        if rows.len().saturating_add(page.observations.len()) as u64 > command.max_rows {
            return Err("dataset_response_too_large".to_string());
        }
        rows.extend(page.observations);
        for (key, value) in page.upstream_refs {
            upstream_refs.insert(format!("page_{page_index}_{key}"), value);
        }
        match page.next_page_token {
            Some(next) if seen_tokens.insert(next.clone()) => page_token = Some(next),
            Some(_) => return Err("dataset_source_unavailable".to_string()),
            None => {
                return Ok(AcquiredRows {
                    source_class: source_class.unwrap_or(DatasetSourceClass::Production),
                    rows,
                    upstream_refs,
                })
            }
        }
    }
    Err("dataset_response_too_large".to_string())
}

fn resolve_source(
    service: &TradeAssemblyService,
    command: &DatasetIngestionCommand,
    request_hash: &str,
    ingestion_idempotency_key: &str,
) -> Result<ResolvedDatasetSource, String> {
    if let Some(revision_id) = &command.supplied_capability_graph_revision_id {
        let current = capability_graph::check_current(
            service,
            json!({
                "revisionId": revision_id,
                "evaluationEpoch": now_rfc3339(service.runtime().clock.now_ms()),
            }),
        );
        if !current["current"].as_bool().unwrap_or(false) {
            return Err("dataset_capability_revision_stale".to_string());
        }
    }
    let plugin = providers::plugin_catalog(service)
        .into_iter()
        .find(|plugin| plugin.instance_ref == command.plugin_instance_ref)
        .ok_or_else(|| "dataset_source_binding_required".to_string())?;
    if !plugin.enabled || plugin.plugin_ref != command.plugin_ref {
        return Err("dataset_source_binding_required".to_string());
    }
    let operation = plugin
        .operations
        .iter()
        .find(|operation| operation.id == command.operation_id)
        .ok_or_else(|| "dataset_operation_unsupported".to_string())?;
    let requirement = providers::requirement_from_body(&json!({
        "requirementId": REQUIREMENT_ID,
        "capability": operation.capability,
        "mode": "research",
        "declaration": "required",
        "purpose": "historical_dataset_acquisition",
        "requiredFor": ["research"],
        "stageRefs": ["inputs"],
        "substepRefs": ["acquire_historical_data"],
        "constraints": {
            "instrumentFamilies": [instrument_family(&command.asset_class)],
            "dataShapes": [data_shape(&command.data_kind)],
            "operations": ["read"],
            "fields": operation.traits.fields,
            "schemaRefs": operation.traits.output_schema_refs,
            "timeframe": command.granularity,
            "deterministic": operation.traits.deterministic,
            "replayable": operation.traits.replayable,
        },
        "fallbackPolicy": "none",
        "policyTags": ["immutable_dataset"],
    }));
    let binding = CapabilityBinding {
        plugin_instance_ref: Some(plugin.instance_ref.clone()),
        plugin_ref: Some(plugin.plugin_ref.clone()),
        operation_id: Some(operation.id.clone()),
        account_ref: None,
    };
    let evaluation_epoch = now_rfc3339(service.runtime().clock.now_ms());
    let revision = if let Some(revision_id) = &command.supplied_capability_graph_revision_id {
        let read = capability_graph::get_revision(service, revision_id);
        let revision = read
            .get("revision")
            .cloned()
            .ok_or_else(|| blocker_code(&read, "dataset_capability_revision_stale"))?;
        let current = capability_graph::check_current(
            service,
            json!({"revisionId": revision_id, "evaluationEpoch": evaluation_epoch}),
        );
        if !current["current"].as_bool().unwrap_or(false) {
            return Err("dataset_capability_revision_stale".to_string());
        }
        revision
    } else {
        let version = strategy::strategy_versions(service, &command.strategy_id)
            .into_iter()
            .find(|version| version["id"].as_str() == Some(command.strategy_version_id.as_str()))
            .ok_or_else(|| "strategy_version_mismatch".to_string())?;
        let strategy_spec_hash = version["specHash"]
            .as_str()
            .ok_or_else(|| "strategy_version_mismatch".to_string())?;
        let saved = capability_graph::save_revision(
            service,
            json!({
                "configId": format!("dataset-ingestion:{request_hash}"),
                "strategyId": command.strategy_id,
                "strategyVersionId": command.strategy_version_id,
                "strategySpecHash": strategy_spec_hash,
                "mode": "research",
                "evaluationEpoch": evaluation_epoch,
                "graphRequest": {
                    "requirements": [requirement],
                    "bindings": {(REQUIREMENT_ID): binding},
                    "configuredFallbacks": {},
                    "evaluationEpoch": evaluation_epoch,
                },
                "idempotencyKey": format!(
                    "dataset.capability:{}",
                    hash_string(ingestion_idempotency_key)
                ),
            }),
        );
        saved
            .get("revision")
            .cloned()
            .ok_or_else(|| blocker_code(&saved, "dataset_source_binding_required"))?
    };
    if revision["strategyId"].as_str() != Some(command.strategy_id.as_str())
        || revision["strategyVersionId"].as_str() != Some(command.strategy_version_id.as_str())
        || revision["mode"].as_str() != Some("research")
    {
        return Err("dataset_capability_revision_stale".to_string());
    }
    capability_graph::require_complete_requirements(&revision, &[requirement])
        .map_err(|_| "dataset_capability_revision_stale".to_string())?;
    capability_graph::require_requested_binding(&revision, REQUIREMENT_ID, &binding)
        .map_err(|_| "dataset_capability_revision_stale".to_string())?;
    if !revision["graph"]["ok"].as_bool().unwrap_or(false) {
        return Err(blocker_code(
            &revision["graph"],
            "dataset_source_binding_required",
        ));
    }
    let selected = revision["graph"]["nodes"]
        .as_array()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|node| node["nodeId"].as_str() == Some(REQUIREMENT_ID))
        })
        .and_then(|node| node.get("selected"))
        .ok_or_else(|| "dataset_source_binding_required".to_string())?;
    if selected["pluginInstanceRef"].as_str() != Some(command.plugin_instance_ref.as_str())
        || selected["pluginRef"].as_str() != Some(command.plugin_ref.as_str())
        || selected["operationId"].as_str() != Some(command.operation_id.as_str())
    {
        return Err("dataset_source_binding_required".to_string());
    }
    Ok(ResolvedDatasetSource {
        plugin_instance_ref: command.plugin_instance_ref.clone(),
        plugin_ref: command.plugin_ref.clone(),
        operation_id: command.operation_id.clone(),
        plugin_manifest_fingerprint: selected["manifestFingerprint"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "dataset_source_binding_required".to_string())?
            .to_string(),
        capability_graph_revision_id: revision["revisionId"]
            .as_str()
            .ok_or_else(|| "dataset_capability_revision_stale".to_string())?
            .to_string(),
        capability_graph_fingerprint: revision["graphFingerprint"]
            .as_str()
            .ok_or_else(|| "dataset_capability_revision_stale".to_string())?
            .to_string(),
        credential_handle: operation
            .credential_grant_required
            .then(|| command.plugin_instance_ref.clone()),
    })
}

fn parse_command(
    service: &TradeAssemblyService,
    body: &Value,
) -> Result<DatasetIngestionCommand, String> {
    let strategy_id = string(body, &["strategyId", "strategy_id"])?;
    let requested_version = optional_string(body, &["strategyVersionId", "strategy_version_id"]);
    let version = requested_version.as_ref().map_or_else(
        || strategy::latest_version(service, &strategy_id),
        |version_id| {
            strategy::strategy_versions(service, &strategy_id)
                .into_iter()
                .find(|version| version["id"].as_str() == Some(version_id))
                .unwrap_or(Value::Null)
        },
    );
    let strategy_version_id = version["id"]
        .as_str()
        .ok_or_else(|| "strategy_version_mismatch".to_string())?
        .to_string();
    let plugin_instance_ref = string(
        body,
        &[
            "pluginInstanceRef",
            "plugin_instance_ref",
            "sourcePluginInstanceRef",
        ],
    )?;
    let plugin = providers::plugin_catalog(service)
        .into_iter()
        .find(|plugin| plugin.instance_ref == plugin_instance_ref)
        .ok_or_else(|| "dataset_source_binding_required".to_string())?;
    let data_kind = enum_value::<HistoricalDataKind>(body, &["dataKind", "data_kind"])?;
    let operation_id = string(body, &["operationId", "operation_id"])?;
    let asset_class = enum_value::<AssetClass>(body, &["assetClass", "asset_class"])?;
    let instruments = body
        .get("instruments")
        .and_then(Value::as_array)
        .ok_or_else(|| "dataset_request_invalid".to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| "dataset_request_invalid".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut time_slice: DatasetTimeSlice = serde_json::from_value(
        body.get("timeSlice")
            .or_else(|| body.get("time_slice"))
            .cloned()
            .ok_or_else(|| "dataset_time_slice_invalid".to_string())?,
    )
    .map_err(|_| "dataset_time_slice_invalid".to_string())?;
    let canonical_start = parse_time(&time_slice.start)?;
    let canonical_end = parse_time(&time_slice.end)?;
    if canonical_start >= canonical_end || instruments.is_empty() {
        return Err("dataset_time_slice_invalid".to_string());
    }
    time_slice.start = canonical_time(canonical_start);
    time_slice.end = canonical_time(canonical_end);
    let normalization_policy: DatasetNormalizationPolicy = serde_json::from_value(
        body.get("normalizationPolicy")
            .or_else(|| body.get("normalization_policy"))
            .cloned()
            .ok_or_else(|| "dataset_request_invalid".to_string())?,
    )
    .map_err(|_| "dataset_request_invalid".to_string())?;
    let quality_policy: DatasetQualityPolicy = serde_json::from_value(
        body.get("qualityPolicy")
            .or_else(|| body.get("quality_policy"))
            .cloned()
            .ok_or_else(|| "dataset_request_invalid".to_string())?,
    )
    .map_err(|_| "dataset_request_invalid".to_string())?;
    if normalization_policy.timestamp_unit != "rfc3339"
        || normalization_policy.timezone != "UTC"
        || !matches!(
            normalization_policy.duplicate_policy.as_str(),
            "reject" | "keep_first" | "keep_last"
        )
    {
        return Err("dataset_request_invalid".to_string());
    }
    let max_rows = body
        .get("maxRows")
        .or_else(|| body.get("max_rows"))
        .and_then(Value::as_u64)
        .unwrap_or(100_000);
    if max_rows == 0 || max_rows > MAX_TOTAL_ROWS {
        return Err("dataset_response_too_large".to_string());
    }
    Ok(DatasetIngestionCommand {
        authority_actor: actor(body),
        strategy_id,
        strategy_version_id,
        plugin_instance_ref,
        plugin_ref: plugin.plugin_ref,
        operation_id,
        asset_class,
        instruments,
        data_kind,
        granularity: string(body, &["granularity"])?,
        time_slice,
        calendar: string(body, &["calendar"])?,
        timezone: string(body, &["timezone"])?,
        normalization_policy,
        quality_policy,
        max_rows,
        supplied_capability_graph_revision_id: optional_string(
            body,
            &["capabilityGraphRevisionId", "capability_graph_revision_id"],
        ),
    })
}

fn normalize_and_validate(
    command: &DatasetIngestionCommand,
    rows: Vec<HistoricalObservation>,
) -> Result<(Vec<HistoricalObservation>, Vec<DatasetQualityFinding>), String> {
    if rows.is_empty() {
        return Err("dataset_source_unavailable".to_string());
    }
    let start = parse_time(&command.time_slice.start)?;
    let end = parse_time(&command.time_slice.end)?;
    let mut normalized = rows
        .into_iter()
        .map(|mut row| {
            let timestamp = parse_time(&row.timestamp)?;
            if timestamp < start || timestamp > end {
                return Err("dataset_row_out_of_range".to_string());
            }
            row.timestamp = canonical_time(timestamp);
            Ok(row)
        })
        .collect::<Result<Vec<_>, String>>()?;
    normalized.sort_by(|left, right| {
        (&left.instrument_id, &left.timestamp).cmp(&(&right.instrument_id, &right.timestamp))
    });
    normalized =
        apply_duplicate_policy(normalized, &command.normalization_policy.duplicate_policy)?;
    let mut findings = Vec::new();
    check_calendar(command, &mut findings)?;
    check_intervals(command, &normalized, &mut findings)?;
    check_invalid_markets(command, &normalized, &mut findings);
    check_outliers(command, &normalized, &mut findings)?;
    check_derivative_fields(command, &normalized, &mut findings)?;
    if findings
        .iter()
        .any(|finding| finding.severity == DatasetQualitySeverity::Error)
    {
        return Err("dataset_quality_rejected".to_string());
    }
    Ok((normalized, findings))
}

fn apply_duplicate_policy(
    rows: Vec<HistoricalObservation>,
    policy: &str,
) -> Result<Vec<HistoricalObservation>, String> {
    let mut result: Vec<HistoricalObservation> = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(previous) = result.last_mut() {
            if previous.instrument_id == row.instrument_id && previous.timestamp == row.timestamp {
                match policy {
                    "reject" => return Err("dataset_rows_unordered".to_string()),
                    "keep_first" => continue,
                    "keep_last" => {
                        *previous = row;
                        continue;
                    }
                    _ => return Err("dataset_request_invalid".to_string()),
                }
            }
        }
        result.push(row);
    }
    Ok(result)
}

fn check_calendar(
    command: &DatasetIngestionCommand,
    findings: &mut Vec<DatasetQualityFinding>,
) -> Result<(), String> {
    let valid = match command.asset_class {
        AssetClass::CryptoSpot => command.calendar == "crypto_24x7" && command.timezone == "UTC",
        AssetClass::Equity | AssetClass::Etf | AssetClass::Option => {
            command.calendar == "XNYS" && command.timezone == "America/New_York"
        }
    };
    if !valid {
        push_finding(
            findings,
            "calendar_mismatch",
            &command.quality_policy.calendar_mismatch,
            None,
            Some(command.time_slice.start.clone()),
            Some(command.time_slice.end.clone()),
            "Declared calendar or timezone is incompatible with the asset class.",
        );
    }
    Ok(())
}

fn check_intervals(
    command: &DatasetIngestionCommand,
    rows: &[HistoricalObservation],
    findings: &mut Vec<DatasetQualityFinding>,
) -> Result<(), String> {
    if command.data_kind == HistoricalDataKind::OptionChain {
        let underlyings = rows
            .iter()
            .filter_map(|row| match &row.data {
                HistoricalObservationData::OptionContract(option) => {
                    Some(option.underlying_instrument_id.as_str())
                }
                HistoricalObservationData::Bar(_) => None,
            })
            .collect::<BTreeSet<_>>();
        for instrument in &command.instruments {
            if !underlyings.contains(instrument.as_str()) {
                push_finding(
                    findings,
                    "missing_intervals",
                    &command.quality_policy.missing_intervals,
                    Some(instrument.clone()),
                    Some(command.time_slice.start.clone()),
                    Some(command.time_slice.end.clone()),
                    "No option observations were returned for the underlying instrument.",
                );
            }
        }
        return Ok(());
    }
    let expected = interval(&command.granularity)?;
    let requested_start = parse_time(&command.time_slice.start)?;
    let mut previous = BTreeMap::<&str, DateTime<Utc>>::new();
    for row in rows {
        let current = parse_time(&row.timestamp)?;
        if let Some(prior) = previous.insert(&row.instrument_id, current) {
            if current - prior > expected {
                push_finding(
                    findings,
                    "missing_intervals",
                    &command.quality_policy.missing_intervals,
                    Some(row.instrument_id.clone()),
                    Some(canonical_time(prior)),
                    Some(canonical_time(current)),
                    "One or more expected intervals are absent.",
                );
            }
        } else if current > requested_start {
            push_finding(
                findings,
                "missing_intervals",
                &command.quality_policy.missing_intervals,
                Some(row.instrument_id.clone()),
                Some(command.time_slice.start.clone()),
                Some(canonical_time(current)),
                "One or more leading intervals are absent.",
            );
        }
    }
    for instrument in &command.instruments {
        let Some(last) = previous.get(instrument.as_str()) else {
            push_finding(
                findings,
                "missing_intervals",
                &command.quality_policy.missing_intervals,
                Some(instrument.clone()),
                Some(command.time_slice.start.clone()),
                Some(command.time_slice.end.clone()),
                "No observations were returned for the instrument.",
            );
            continue;
        };
        let requested_end = parse_time(&command.time_slice.end)?;
        if requested_end - *last > expected {
            push_finding(
                findings,
                "stale_observations",
                &command.quality_policy.stale_observations,
                Some(instrument.clone()),
                Some(canonical_time(*last)),
                Some(canonical_time(requested_end)),
                "The final observation is stale relative to the requested time slice.",
            );
        }
    }
    Ok(())
}

fn check_invalid_markets(
    command: &DatasetIngestionCommand,
    rows: &[HistoricalObservation],
    findings: &mut Vec<DatasetQualityFinding>,
) {
    for row in rows {
        let invalid = match &row.data {
            HistoricalObservationData::Bar(bar) => {
                let highest_body = bar.open.max(bar.close);
                let lowest_body = bar.open.min(bar.close);
                bar.open <= 0.0
                    || bar.high <= 0.0
                    || bar.low <= 0.0
                    || bar.close <= 0.0
                    || bar.volume < 0.0
                    || bar.high < highest_body
                    || bar.low > lowest_body
                    || bar.high < bar.low
            }
            HistoricalObservationData::OptionContract(option) => {
                option.strike <= 0.0
                    || option.bid.is_some_and(|value| value < 0.0)
                    || option.ask.is_some_and(|value| value < 0.0)
                    || option.last.is_some_and(|value| value < 0.0)
                    || option.volume.is_some_and(|value| value < 0.0)
                    || option.open_interest.is_some_and(|value| value < 0.0)
                    || matches!((option.bid, option.ask), (Some(bid), Some(ask)) if ask < bid)
            }
        };
        if invalid {
            push_finding(
                findings,
                "invalid_market",
                &command.quality_policy.invalid_markets,
                Some(row.instrument_id.clone()),
                Some(row.timestamp.clone()),
                Some(row.timestamp.clone()),
                "Observation contains an invalid or crossed market value.",
            );
        }
    }
}

fn check_outliers(
    command: &DatasetIngestionCommand,
    rows: &[HistoricalObservation],
    findings: &mut Vec<DatasetQualityFinding>,
) -> Result<(), String> {
    let mut previous_close = BTreeMap::<&str, f64>::new();
    for row in rows {
        let HistoricalObservationData::Bar(bar) = &row.data else {
            continue;
        };
        if let Some(previous) = previous_close.insert(&row.instrument_id, bar.close) {
            if previous > 0.0 && ((bar.close - previous) / previous).abs() > 0.5 {
                push_finding(
                    findings,
                    "price_outlier",
                    &command.quality_policy.outliers,
                    Some(row.instrument_id.clone()),
                    Some(row.timestamp.clone()),
                    Some(row.timestamp.clone()),
                    "Adjacent close prices differ by more than fifty percent.",
                );
            }
        }
    }
    Ok(())
}

fn check_derivative_fields(
    command: &DatasetIngestionCommand,
    rows: &[HistoricalObservation],
    findings: &mut Vec<DatasetQualityFinding>,
) -> Result<(), String> {
    if command.data_kind != HistoricalDataKind::OptionChain {
        return Ok(());
    }
    for row in rows {
        let HistoricalObservationData::OptionContract(option) = &row.data else {
            return Err("dataset_schema_unsupported".to_string());
        };
        if option.bid.is_none()
            || option.ask.is_none()
            || option.open_interest.is_none()
            || option.implied_volatility.is_none()
        {
            push_finding(
                findings,
                "missing_derivative_fields",
                &command.quality_policy.missing_derivative_fields,
                Some(row.instrument_id.clone()),
                Some(row.timestamp.clone()),
                Some(row.timestamp.clone()),
                "Option observation lacks one or more configured research fields.",
            );
        }
    }
    Ok(())
}

fn push_finding(
    findings: &mut Vec<DatasetQualityFinding>,
    code: &str,
    disposition: &QualityDisposition,
    instrument_id: Option<String>,
    start: Option<String>,
    end: Option<String>,
    detail: &str,
) {
    if *disposition == QualityDisposition::Allow {
        return;
    }
    findings.push(DatasetQualityFinding {
        code: code.to_string(),
        severity: if *disposition == QualityDisposition::Reject {
            DatasetQualitySeverity::Error
        } else {
            DatasetQualitySeverity::Warning
        },
        instrument_id,
        start,
        end,
        detail: detail.to_string(),
    });
}

fn transition(
    service: &TradeAssemblyService,
    record: &mut DatasetIngestionRecord,
    next: DatasetIngestionState,
    context: &SideEffectContext,
    code: Option<&str>,
) -> Result<(), String> {
    if !valid_transition(&record.aggregate.state, &next) {
        return Err("dataset_ingestion_transition_invalid".to_string());
    }
    let previous = record.clone();
    record.aggregate.state = next;
    record.aggregate.updated_at_ms = service.runtime().clock.now_ms();
    if let Some(code) = code {
        if !record.aggregate.blockers.iter().any(|value| value == code) {
            record.aggregate.blockers.push(code.to_string());
        }
    }
    replace_record(service, &previous, record, context)?;
    record_evidence(service, record, context)
}

fn valid_transition(current: &DatasetIngestionState, next: &DatasetIngestionState) -> bool {
    current == next
        || matches!(
            (current, next),
            (
                DatasetIngestionState::Requested,
                DatasetIngestionState::ResolvingSource
            ) | (
                DatasetIngestionState::ResolvingSource,
                DatasetIngestionState::Ingesting
            ) | (
                DatasetIngestionState::Ingesting,
                DatasetIngestionState::Validating
            ) | (
                DatasetIngestionState::Validating,
                DatasetIngestionState::Completed
            ) | (
                DatasetIngestionState::Requested
                    | DatasetIngestionState::ResolvingSource
                    | DatasetIngestionState::Ingesting
                    | DatasetIngestionState::Validating,
                DatasetIngestionState::Blocked
                    | DatasetIngestionState::Failed
                    | DatasetIngestionState::Canceled
            ) | (
                DatasetIngestionState::ResolvingSource
                    | DatasetIngestionState::Ingesting
                    | DatasetIngestionState::Validating,
                DatasetIngestionState::ResolvingSource
            )
        )
}

fn persist_record(
    service: &TradeAssemblyService,
    record: &DatasetIngestionRecord,
    context: &SideEffectContext,
) -> Result<(), String> {
    let value = serde_json::to_value(record)
        .map_err(|_| "dataset_ingestion_persistence_failed".to_string())?;
    service
        .runtime()
        .storage
        .put_json(
            INGESTIONS_NS,
            &record.aggregate.ingestion_id,
            value,
            context,
        )
        .map_err(|_| "dataset_ingestion_persistence_failed".to_string())
}

fn replace_record(
    service: &TradeAssemblyService,
    expected: &DatasetIngestionRecord,
    replacement: &DatasetIngestionRecord,
    context: &SideEffectContext,
) -> Result<(), String> {
    let replacement = serde_json::to_value(replacement)
        .map_err(|_| "dataset_ingestion_persistence_failed".to_string())?;
    let ingestion_id = replacement["aggregate"]["ingestionId"]
        .as_str()
        .ok_or_else(|| "dataset_ingestion_persistence_failed".to_string())?
        .to_string();
    let current_value = service
        .runtime()
        .storage
        .get_json(INGESTIONS_NS, &ingestion_id)
        .map_err(|_| "dataset_ingestion_persistence_failed".to_string())?
        .ok_or_else(|| "dataset_ingestion_not_found".to_string())?;
    let current: DatasetIngestionRecord = serde_json::from_value(current_value.clone())
        .map_err(|_| "dataset_ingestion_integrity_failed".to_string())?;
    if current.aggregate.state == DatasetIngestionState::Canceled {
        return Err("dataset_ingestion_canceled".to_string());
    }
    if current.aggregate.state != expected.aggregate.state
        || current.aggregate.attempt != expected.aggregate.attempt
        || current.aggregate.fencing_token != expected.aggregate.fencing_token
    {
        return Err("dataset_ingestion_conflict".to_string());
    }
    match service.runtime().storage.compare_and_put_json(
        INGESTIONS_NS,
        &ingestion_id,
        current_value,
        replacement,
        context,
    ) {
        Ok(ComparePutOutcome::Updated) => Ok(()),
        Ok(ComparePutOutcome::Conflict) => {
            let current = read_record(service, &ingestion_id)?;
            if current.aggregate.state == DatasetIngestionState::Canceled {
                Err("dataset_ingestion_canceled".to_string())
            } else {
                Err("dataset_ingestion_conflict".to_string())
            }
        }
        Err(_) => Err("dataset_ingestion_persistence_failed".to_string()),
    }
}

fn ensure_record_current(
    service: &TradeAssemblyService,
    expected: &DatasetIngestionRecord,
) -> Result<(), String> {
    let current = read_record(service, &expected.aggregate.ingestion_id)?;
    if current.aggregate.state == DatasetIngestionState::Canceled {
        return Err("dataset_ingestion_canceled".to_string());
    }
    if current.aggregate.state != expected.aggregate.state
        || current.aggregate.attempt != expected.aggregate.attempt
        || current.aggregate.fencing_token != expected.aggregate.fencing_token
    {
        return Err("dataset_ingestion_conflict".to_string());
    }
    Ok(())
}

fn read_record(
    service: &TradeAssemblyService,
    ingestion_id: &str,
) -> Result<DatasetIngestionRecord, String> {
    let value = service
        .runtime()
        .storage
        .get_json(INGESTIONS_NS, ingestion_id)
        .map_err(|_| "dataset_ingestion_persistence_failed".to_string())?
        .ok_or_else(|| "dataset_ingestion_not_found".to_string())?;
    serde_json::from_value(value).map_err(|_| "dataset_ingestion_integrity_failed".to_string())
}

fn record_evidence(
    service: &TradeAssemblyService,
    record: &DatasetIngestionRecord,
    context: &SideEffectContext,
) -> Result<(), String> {
    let state = serde_json::to_value(&record.aggregate.state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let evidence_key = IdempotencyKey::new(format!(
        "{}:{}:{}:{}",
        record.aggregate.idempotency_key,
        record.aggregate.attempt,
        record.aggregate.fencing_token,
        state
    ))?;
    let evidence_context = SideEffectContext::new(context.authority.clone(), evidence_key.clone());
    service
        .runtime()
        .evidence
        .append(EvidenceRecord {
            evidence_id: format!("evidence_{}", &hash_string(evidence_key.as_str())[..24]),
            evidence_type: "dataset.ingestion.state_changed".to_string(),
            aggregate_id: record.aggregate.ingestion_id.clone(),
            payload: json!({
                "ingestionId": record.aggregate.ingestion_id,
                "state": record.aggregate.state,
                "attempt": record.aggregate.attempt,
                "fencingToken": record.aggregate.fencing_token,
                "requestHash": record.aggregate.request_hash,
                "snapshotId": record.aggregate.snapshot_id,
                "blockers": record.aggregate.blockers,
                "source": record.source.as_ref().map(|source| json!({
                    "pluginInstanceRef": source.plugin_instance_ref,
                    "pluginRef": source.plugin_ref,
                    "operationId": source.operation_id,
                    "manifestFingerprint": source.plugin_manifest_fingerprint,
                    "capabilityGraphRevisionId": source.capability_graph_revision_id,
                    "capabilityGraphFingerprint": source.capability_graph_fingerprint,
                })),
                "noAdvice": true,
            }),
            idempotency_key: evidence_key,
            recorded_at_ms: service.runtime().clock.now_ms(),
        })
        .map_err(|_| "dataset_ingestion_evidence_failed".to_string())?;
    service
        .runtime()
        .record_side_effect(
            "dataset.ingestion.state_changed",
            json!({
                "ingestionId": record.aggregate.ingestion_id,
                "state": record.aggregate.state,
                "requestHash": record.aggregate.request_hash,
                "snapshotId": record.aggregate.snapshot_id,
                "noAdvice": true,
            }),
            &evidence_context,
        )
        .map(|_| ())
        .map_err(|_| "dataset_ingestion_evidence_failed".to_string())
}

fn response(service: &TradeAssemblyService, record: &DatasetIngestionRecord) -> Value {
    let snapshot = match record.aggregate.snapshot_id.as_deref() {
        Some(id) => match service.runtime().dataset_snapshots.get(id) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return json!({
                    "status": "failed",
                    "ingestion": record.aggregate,
                    "source": record.source,
                    "snapshot": Value::Null,
                    "error": {"code": "dataset_snapshot_integrity_failed"},
                    "noAdvice": true,
                })
            }
        },
        None => None,
    };
    json!({
        "status": state_name(&record.aggregate.state),
        "ingestion": record.aggregate,
        "source": record.source,
        "snapshot": snapshot,
        "noAdvice": true,
    })
}

fn not_found() -> Value {
    json!({"status": "not_found", "error": {"code": "dataset_ingestion_not_found"}})
}

fn context(body: &Value, idempotency_key: &str) -> SideEffectContext {
    let authority = body
        .get("authorityContext")
        .or_else(|| body.get("authority_context"));
    SideEffectContext::new(
        AuthorityContext {
            actor: actor(body),
            surface: authority
                .and_then(|value| value.get("surface"))
                .and_then(Value::as_str)
                .or_else(|| body.get("sourceInterface").and_then(Value::as_str))
                .unwrap_or("local")
                .to_string(),
            account_mode: "research".to_string(),
        },
        IdempotencyKey::new(idempotency_key).expect("validated idempotency key"),
    )
}

fn actor(body: &Value) -> String {
    body.get("authorityContext")
        .or_else(|| body.get("authority_context"))
        .and_then(|value| value.get("actor"))
        .and_then(Value::as_str)
        .or_else(|| {
            body.get("actor")
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
        })
        .unwrap_or("local-user")
        .to_string()
}

fn validate_source_class(
    source: &ResolvedDatasetSource,
    source_class: &DatasetSourceClass,
) -> Result<(), String> {
    match source.plugin_ref.as_str() {
        "tradeassembly.local-data" if *source_class == DatasetSourceClass::Test => Ok(()),
        "tradeassembly.local-data" => Err("dataset_source_binding_required".to_string()),
        _ if *source_class == DatasetSourceClass::Production => Ok(()),
        _ => Err("dataset_source_binding_required".to_string()),
    }
}

fn stable_source_error(error: String) -> String {
    const CODES: &[&str] = &[
        "dataset_request_invalid",
        "dataset_time_slice_invalid",
        "dataset_source_unavailable",
        "dataset_operation_unsupported",
        "dataset_credential_missing",
        "dataset_schema_unsupported",
        "dataset_instrument_unsupported",
        "dataset_response_too_large",
        "dataset_row_invalid",
    ];
    if CODES.contains(&error.as_str()) {
        error
    } else {
        "dataset_source_unavailable".to_string()
    }
}

fn stable_snapshot_error(error: String) -> String {
    if error.starts_with("dataset_snapshot_") {
        error
    } else {
        "dataset_snapshot_write_conflict".to_string()
    }
}

fn failure_state(code: &str) -> &'static str {
    if matches!(
        code,
        "dataset_source_binding_required"
            | "dataset_capability_revision_stale"
            | "dataset_credential_missing"
    ) {
        "blocked"
    } else {
        "failed"
    }
}

fn failure_state_enum(code: &str) -> DatasetIngestionState {
    if failure_state(code) == "blocked" {
        DatasetIngestionState::Blocked
    } else {
        DatasetIngestionState::Failed
    }
}

fn blocker_code(value: &Value, fallback: &str) -> String {
    value
        .get("blockers")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .map(|code| match code {
            "resolution_stale" | "capability_revision_mismatch" => {
                "dataset_capability_revision_stale"
            }
            "credential_missing" | "credential_not_configured" => "dataset_credential_missing",
            _ => fallback,
        })
        .unwrap_or(fallback)
        .to_string()
}

fn state_name(state: &DatasetIngestionState) -> &'static str {
    match state {
        DatasetIngestionState::Requested => "requested",
        DatasetIngestionState::ResolvingSource => "resolving_source",
        DatasetIngestionState::Ingesting => "ingesting",
        DatasetIngestionState::Validating => "validating",
        DatasetIngestionState::Completed => "completed",
        DatasetIngestionState::Blocked => "blocked",
        DatasetIngestionState::Failed => "failed",
        DatasetIngestionState::Canceled => "canceled",
    }
}

fn string(body: &Value, keys: &[&str]) -> Result<String, String> {
    optional_string(body, keys).ok_or_else(|| "dataset_request_invalid".to_string())
}

fn optional_string(body: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| body.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn enum_value<T>(body: &Value, keys: &[&str]) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let value = optional_string(body, keys).ok_or_else(|| "dataset_request_invalid".to_string())?;
    serde_json::from_value(json!(value)).map_err(|_| "dataset_request_invalid".to_string())
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| "dataset_time_slice_invalid".to_string())
}

fn canonical_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn now_rfc3339(now_ms: i64) -> String {
    Utc.timestamp_millis_opt(now_ms)
        .single()
        .unwrap_or_else(|| Utc.timestamp_millis_opt(0).single().expect("unix epoch"))
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn interval(value: &str) -> Result<ChronoDuration, String> {
    if value.len() < 2 {
        return Err("dataset_request_invalid".to_string());
    }
    let (number, unit) = value.split_at(value.len() - 1);
    let number = number
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "dataset_request_invalid".to_string())?;
    match unit {
        "m" => Ok(ChronoDuration::minutes(number)),
        "h" => Ok(ChronoDuration::hours(number)),
        "d" => Ok(ChronoDuration::days(number)),
        _ => Err("dataset_request_invalid".to_string()),
    }
}

fn instrument_family(asset_class: &AssetClass) -> &'static str {
    match asset_class {
        AssetClass::Equity | AssetClass::Etf => "equity",
        AssetClass::CryptoSpot => "crypto_spot",
        AssetClass::Option => "option_contract",
    }
}

fn data_shape(data_kind: &HistoricalDataKind) -> &'static str {
    match data_kind {
        HistoricalDataKind::Bars => "bars",
        HistoricalDataKind::OptionChain => "option_chain",
    }
}

fn hash_canonical<T: Serialize>(value: &T) -> Result<String, String> {
    let bytes = serde_json_canonicalizer::to_vec(value)
        .map_err(|_| "dataset_request_invalid".to_string())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn hash_string(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::local;
    use crate::historical_data::{BarObservation, HistoricalObservationData};
    use crate::ports::{
        FailureMode, HistoricalDataPage, HistoricalDataPort, PortDescriptor, PortKind,
        VersionedPort,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    struct CountingSource {
        calls: Arc<AtomicUsize>,
        source_class: DatasetSourceClass,
    }

    struct FailingSource;

    impl VersionedPort for FailingSource {
        fn descriptors(&self) -> Vec<PortDescriptor> {
            vec![
                PortDescriptor::new(PortKind::Plugins, "test.failing-historical-data")
                    .for_profiles(&["local"])
                    .with_capabilities(&["dataset.historical.acquire"]),
            ]
        }
    }

    impl HistoricalDataPort for FailingSource {
        fn acquire_page(
            &self,
            _request: &HistoricalDataRequest,
            _context: &SideEffectContext,
        ) -> Result<HistoricalDataPage, String> {
            Err("upstream rejected api_key=super-secret-value".to_string())
        }
    }

    impl VersionedPort for CountingSource {
        fn descriptors(&self) -> Vec<PortDescriptor> {
            let mut descriptor = PortDescriptor::new(PortKind::Plugins, "test.historical-data")
                .for_profiles(&["local"])
                .with_capabilities(&["dataset.historical.acquire"]);
            descriptor.failure_mode = FailureMode::FailClosed;
            vec![descriptor]
        }
    }

    impl HistoricalDataPort for CountingSource {
        fn acquire_page(
            &self,
            request: &HistoricalDataRequest,
            _context: &SideEffectContext,
        ) -> Result<crate::ports::HistoricalDataPage, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(HistoricalDataPage {
                source_class: self.source_class.clone(),
                observations: vec![
                    row(&request.instruments[0], "2026-01-02T00:00:00Z", 100.0),
                    row(&request.instruments[0], "2026-01-02T00:01:00Z", 101.0),
                    row(&request.instruments[0], "2026-01-02T00:02:00Z", 102.0),
                ],
                next_page_token: None,
                upstream_refs: BTreeMap::from([("request_id".to_string(), "test-1".to_string())]),
            })
        }
    }

    fn row(instrument: &str, timestamp: &str, close: f64) -> HistoricalObservation {
        HistoricalObservation {
            instrument_id: instrument.to_string(),
            timestamp: timestamp.to_string(),
            data: HistoricalObservationData::Bar(BarObservation {
                open: close,
                high: close + 1.0,
                low: close - 1.0,
                close,
                volume: 100.0,
            }),
        }
    }

    fn body(idempotency_key: &str) -> Value {
        json!({
            "strategyId": "strat_local_btc_demo",
            "pluginInstanceRef": "local-data",
            "operationId": "marketdata.bars.read_v1",
            "assetClass": "crypto_spot",
            "instruments": ["BTC/USD"],
            "dataKind": "bars",
            "granularity": "1m",
            "timeSlice": {"start": "2026-01-02T00:00:00Z", "end": "2026-01-02T00:02:00Z"},
            "calendar": "crypto_24x7",
            "timezone": "UTC",
            "normalizationPolicy": {
                "timestampUnit": "rfc3339",
                "timezone": "UTC",
                "duplicatePolicy": "reject",
                "priceAdjustment": "raw"
            },
            "qualityPolicy": {
                "missingIntervals": "reject",
                "staleObservations": "reject",
                "outliers": "warn",
                "invalidMarkets": "reject",
                "calendarMismatch": "reject",
                "corporateActionGaps": "warn",
                "missingDerivativeFields": "reject"
            },
            "maxRows": 100,
            "idempotencyKey": idempotency_key,
            "authorityContext": {"actor": "user.local", "surface": "test"}
        })
    }

    fn service(temp: &TempDir, source: Arc<dyn HistoricalDataPort>) -> TradeAssemblyService {
        let db = temp
            .path()
            .join("tradeassembly.db")
            .to_string_lossy()
            .to_string();
        let mut runtime = local::test_runtime(db.clone());
        runtime.historical_data = source;
        TradeAssemblyService::from_runtime(db, runtime)
    }

    fn default_service(temp: &TempDir) -> TradeAssemblyService {
        let db = temp
            .path()
            .join("tradeassembly.db")
            .to_string_lossy()
            .to_string();
        TradeAssemblyService::from_runtime(db.clone(), local::test_runtime(db))
    }

    #[test]
    fn exact_binding_builds_an_immutable_snapshot_and_restart_deduplicates() {
        let temp = TempDir::new().expect("temp");
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::clone(&calls),
            source_class: DatasetSourceClass::Test,
        });
        let first = service(&temp, Arc::clone(&source));
        let created = create(&first, body("dataset-f02-restart"));
        assert_eq!(created["status"], "completed", "{created:#}");
        assert_eq!(created["snapshot"]["rowCount"], 3);
        assert_eq!(
            created["snapshot"]["content"]["source"]["sourceClass"],
            "test"
        );
        assert_eq!(created["source"]["pluginInstanceRef"], "local-data");
        assert!(created["source"]["capabilityGraphRevisionId"]
            .as_str()
            .unwrap_or_default()
            .starts_with("caprev_"));
        drop(first);

        let restarted = service(&temp, source);
        let replay = create(&restarted, body("dataset-f02-restart"));
        assert_eq!(replay["status"], "completed");
        assert_eq!(
            replay["snapshot"]["datasetId"],
            created["snapshot"]["datasetId"]
        );
        assert_eq!(list(&restarted).len(), 1);
        let ingestion_id = replay["ingestion"]["ingestionId"].as_str().expect("id");
        assert_eq!(get(&restarted, ingestion_id)["status"], "completed");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn equivalent_requests_with_distinct_idempotency_keys_get_distinct_source_resolution_claims() {
        let temp = TempDir::new().expect("temp");
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::clone(&calls),
            source_class: DatasetSourceClass::Test,
        });
        let service = service(&temp, source);

        let first = create(&service, body("dataset-f02-repeat-a"));
        let second = create(&service, body("dataset-f02-repeat-b"));

        assert_eq!(first["status"], "completed", "{first:#}");
        assert_eq!(second["status"], "completed", "{second:#}");
        assert_ne!(
            first["ingestion"]["ingestionId"],
            second["ingestion"]["ingestionId"]
        );
        assert_eq!(list(&service).len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            service
                .runtime()
                .storage
                .list_json("capability_graph_idempotency")
                .expect("capability resolution claims")
                .len(),
            2
        );
    }

    #[test]
    fn retry_recovers_an_idempotency_claim_persisted_before_the_mutable_record() {
        let temp = TempDir::new().expect("temp");
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::clone(&calls),
            source_class: DatasetSourceClass::Test,
        });
        let service = service(&temp, source);
        let body = body("dataset-f02-crash-window");
        let command = parse_command(&service, &body).expect("command");
        let request_hash = hash_canonical(&command).expect("request hash");
        let idempotency_key = "dataset-f02-crash-window";
        let ingestion_id = format!("ingestion_{}", &hash_string(idempotency_key)[..24]);
        let context = context(&body, idempotency_key);
        service
            .runtime()
            .storage
            .put_json_if_absent(
                IDEMPOTENCY_NS,
                &hash_string(idempotency_key),
                json!({"requestHash": request_hash, "ingestionId": ingestion_id}),
                &context,
            )
            .expect("simulate durable claim before crash");

        let recovered = create(&service, body);
        assert_eq!(recovered["status"], "completed", "{recovered:#}");
        assert_eq!(recovered["ingestion"]["attempt"], 2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn changed_request_with_same_key_conflicts_without_invocation() {
        let temp = TempDir::new().expect("temp");
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::clone(&calls),
            source_class: DatasetSourceClass::Test,
        });
        let service = service(&temp, source);
        assert_eq!(
            create(&service, body("dataset-f02-conflict"))["status"],
            "completed"
        );
        let mut changed = body("dataset-f02-conflict");
        changed["granularity"] = json!("5m");
        let conflict = create(&service, changed);
        assert_eq!(conflict["error"]["code"], "dataset_ingestion_conflict");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn source_class_substitution_and_quality_rejection_fail_closed() {
        let temp = TempDir::new().expect("temp");
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::new(AtomicUsize::new(0)),
            source_class: DatasetSourceClass::Production,
        });
        let service = service(&temp, source);
        let substituted = create(&service, body("dataset-f02-source-class"));
        assert_eq!(substituted["status"], "blocked");
        assert_eq!(
            substituted["ingestion"]["blockers"][0],
            "dataset_source_binding_required"
        );

        let rows = vec![
            row("BTC/USD", "2026-01-02T00:00:00Z", 100.0),
            row("BTC/USD", "2026-01-02T00:02:00Z", 102.0),
        ];
        let command: DatasetIngestionCommand =
            parse_command(&service, &body("quality")).expect("command");
        assert_eq!(
            normalize_and_validate(&command, rows).expect_err("missing minute"),
            "dataset_quality_rejected"
        );

        let leading_gap = vec![
            row("BTC/USD", "2026-01-02T00:01:00Z", 101.0),
            row("BTC/USD", "2026-01-02T00:02:00Z", 102.0),
        ];
        assert_eq!(
            normalize_and_validate(&command, leading_gap).expect_err("missing leading minute"),
            "dataset_quality_rejected"
        );
    }

    #[test]
    fn local_adapter_ingests_typed_equity_crypto_and_option_datasets() {
        let temp = TempDir::new().expect("temp");
        let service = default_service(&temp);

        let mut equity = body("dataset-f02-equity");
        equity["assetClass"] = json!("equity");
        equity["instruments"] = json!(["SPY"]);
        equity["calendar"] = json!("XNYS");
        equity["timezone"] = json!("America/New_York");
        let equity = create(&service, equity);
        assert_eq!(equity["status"], "completed", "{equity:#}");
        assert_eq!(equity["snapshot"]["content"]["assetClasses"][0], "equity");

        let crypto = create(&service, body("dataset-f02-crypto"));
        assert_eq!(crypto["status"], "completed", "{crypto:#}");
        assert_eq!(
            crypto["snapshot"]["content"]["assetClasses"][0],
            "crypto_spot"
        );

        let mut options = body("dataset-f02-options");
        options["operationId"] = json!("marketdata.options_chain.read");
        options["assetClass"] = json!("option");
        options["instruments"] = json!(["SPY"]);
        options["dataKind"] = json!("option_chain");
        options["calendar"] = json!("XNYS");
        options["timezone"] = json!("America/New_York");
        let options = create(&service, options);
        assert_eq!(options["status"], "completed", "{options:#}");
        assert_eq!(options["snapshot"]["content"]["dataKind"], "option_chain");
    }

    #[test]
    fn cancellation_is_monotonic_and_terminal() {
        let temp = TempDir::new().expect("temp");
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::new(AtomicUsize::new(0)),
            source_class: DatasetSourceClass::Test,
        });
        let service = service(&temp, source);
        let command = parse_command(&service, &body("dataset-f02-cancel")).expect("command");
        let now = service.runtime().clock.now_ms();
        let record = DatasetIngestionRecord {
            aggregate: DatasetIngestion {
                ingestion_id: "ingestion_cancel".to_string(),
                idempotency_key: "dataset-f02-cancel".to_string(),
                request_hash: hash_canonical(&command).expect("hash"),
                strategy_id: command.strategy_id.clone(),
                strategy_version_id: Some(command.strategy_version_id.clone()),
                state: DatasetIngestionState::Requested,
                attempt: 1,
                fencing_token: 1,
                requested_at_ms: now,
                updated_at_ms: now,
                snapshot_id: None,
                blockers: vec![],
                diagnostics: vec![],
            },
            command,
            source: None,
        };
        let context = context(&body("dataset-f02-cancel"), "dataset-f02-cancel");
        persist_record(&service, &record, &context).expect("persist");
        let unverified = verify(&service, "ingestion_cancel");
        assert_eq!(unverified["verified"], false);
        assert_eq!(
            unverified["error"]["code"],
            "dataset_snapshot_not_available"
        );
        let mut stale_writer = record.clone();
        let mut wrong_actor = body("dataset-f02-cancel");
        wrong_actor["authorityContext"]["actor"] = json!("other-user");
        assert_eq!(
            cancel(&service, "ingestion_cancel", &wrong_actor)["error"]["code"],
            "dataset_authority_mismatch"
        );
        assert_eq!(
            cancel(&service, "ingestion_cancel", &body("dataset-f02-cancel"))["status"],
            "canceled"
        );
        assert_eq!(
            transition(
                &service,
                &mut stale_writer,
                DatasetIngestionState::ResolvingSource,
                &context,
                None,
            )
            .expect_err("stale writer cannot overwrite cancellation"),
            "dataset_ingestion_canceled"
        );
        assert_eq!(
            read_record(&service, "ingestion_cancel")
                .expect("record")
                .aggregate
                .state,
            DatasetIngestionState::Canceled
        );
        assert_eq!(
            cancel(&service, "ingestion_cancel", &body("dataset-f02-cancel"))["status"],
            "conflict"
        );
    }

    #[test]
    fn mismatched_plugin_operation_binding_fails_before_adapter_invocation() {
        let temp = TempDir::new().expect("temp");
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::clone(&calls),
            source_class: DatasetSourceClass::Production,
        });
        let service = service(&temp, source);
        let mut request = body("dataset-f02-unsupported-operation");
        request["pluginInstanceRef"] = json!("local-data");
        request["operationId"] = json!("marketdata.unadvertised.read");
        request["assetClass"] = json!("option");
        request["instruments"] = json!(["SPY"]);
        request["dataKind"] = json!("option_chain");
        request["calendar"] = json!("XNYS");
        request["timezone"] = json!("America/New_York");

        let result = create(&service, request);
        assert_eq!(result["status"], "blocked", "{result:#}");
        assert_eq!(
            result["ingestion"]["blockers"][0],
            "dataset_operation_unsupported"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn limits_duplicates_and_source_errors_fail_closed_without_secret_evidence() {
        let temp = TempDir::new().expect("temp");
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::new(AtomicUsize::new(0)),
            source_class: DatasetSourceClass::Test,
        });
        let active = service(&temp, source);
        let mut limited = body("dataset-f02-limit");
        limited["maxRows"] = json!(2);
        let limited = create(&active, limited);
        assert_eq!(limited["status"], "failed");
        assert_eq!(
            limited["ingestion"]["blockers"][0],
            "dataset_response_too_large"
        );

        let command = parse_command(&active, &body("dataset-f02-duplicate")).expect("command");
        let duplicate = vec![
            row("BTC/USD", "2026-01-02T00:00:00Z", 100.0),
            row("BTC/USD", "2026-01-02T00:00:00Z", 101.0),
        ];
        assert_eq!(
            normalize_and_validate(&command, duplicate).expect_err("duplicate"),
            "dataset_rows_unordered"
        );

        let failing: Arc<dyn HistoricalDataPort> = Arc::new(FailingSource);
        let failed = service(&temp, failing);
        let outage = create(&failed, body("dataset-f02-outage"));
        assert_eq!(outage["status"], "failed");
        assert_eq!(
            outage["ingestion"]["blockers"][0],
            "dataset_source_unavailable"
        );
        let evidence = failed
            .runtime()
            .evidence
            .list(outage["ingestion"]["ingestionId"].as_str().expect("id"))
            .expect("evidence");
        assert!(!serde_json::to_string(&evidence)
            .expect("json")
            .contains("super-secret-value"));
    }

    #[test]
    fn stale_capability_revision_and_snapshot_tamper_are_visible() {
        let temp = TempDir::new().expect("temp");
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::new(AtomicUsize::new(0)),
            source_class: DatasetSourceClass::Test,
        });
        let service = service(&temp, source);
        let created = create(&service, body("dataset-f02-integrity"));
        assert_eq!(created["status"], "completed", "{created:#}");
        let revision_id = created["source"]["capabilityGraphRevisionId"]
            .as_str()
            .expect("revision")
            .to_string();
        let context = context(&body("disable-local-data"), "disable-local-data");
        service
            .runtime()
            .storage
            .put_json(
                "plugin_instances",
                "plugin:tradeassembly.local-data:enabled",
                json!({"enabled": false}),
                &context,
            )
            .expect("disable");
        let mut stale = body("dataset-f02-stale");
        stale["capabilityGraphRevisionId"] = json!(revision_id);
        let stale = create(&service, stale);
        assert_eq!(stale["status"], "blocked");
        assert_eq!(
            stale["ingestion"]["blockers"][0],
            "dataset_capability_revision_stale"
        );

        service
            .runtime()
            .storage
            .put_json(
                "plugin_instances",
                "plugin:tradeassembly.local-data:enabled",
                json!({"enabled": true}),
                &context,
            )
            .expect("enable");
        let snapshot_id = created["snapshot"]["datasetId"].as_str().expect("snapshot");
        let mut stored = service
            .runtime()
            .storage
            .get_json("dataset_snapshots_v1", snapshot_id)
            .expect("read")
            .expect("snapshot value");
        stored["content"]["observations"][0]["data"]["data"]["close"] = json!(999.0);
        service
            .runtime()
            .storage
            .put_json("dataset_snapshots_v1", snapshot_id, stored, &context)
            .expect("simulate owner tamper");
        let ingestion_id = created["ingestion"]["ingestionId"]
            .as_str()
            .expect("ingestion");
        let tampered = get(&service, ingestion_id);
        assert_eq!(tampered["status"], "failed");
        assert_eq!(
            tampered["error"]["code"],
            "dataset_snapshot_integrity_failed"
        );
        let tampered_verification = verify(&service, ingestion_id);
        assert_eq!(tampered_verification["verified"], false);
        assert_eq!(
            tampered_verification["error"]["code"],
            "dataset_snapshot_integrity_failed"
        );
    }

    #[test]
    fn request_requires_exact_operation_and_revision_constraints() {
        let temp = TempDir::new().expect("temp");
        let calls = Arc::new(AtomicUsize::new(0));
        let source: Arc<dyn HistoricalDataPort> = Arc::new(CountingSource {
            calls: Arc::clone(&calls),
            source_class: DatasetSourceClass::Test,
        });
        let service = service(&temp, source);

        let mut missing_operation = body("dataset-f05-operation-required");
        missing_operation
            .as_object_mut()
            .expect("request object")
            .remove("operationId");
        let missing_operation = create(&service, missing_operation);
        assert_eq!(missing_operation["status"], "failed");
        assert_eq!(
            missing_operation["error"]["code"],
            "dataset_request_invalid"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let created = create(&service, body("dataset-f05-revision-source"));
        assert_eq!(created["status"], "completed", "{created:#}");
        let revision_id = created["source"]["capabilityGraphRevisionId"]
            .as_str()
            .expect("revision")
            .to_string();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut substituted = body("dataset-f05-revision-substitution");
        substituted["granularity"] = json!("5m");
        substituted["capabilityGraphRevisionId"] = json!(revision_id);
        let substituted = create(&service, substituted);
        assert_eq!(substituted["status"], "blocked", "{substituted:#}");
        assert_eq!(
            substituted["ingestion"]["blockers"][0],
            "dataset_capability_revision_stale"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
