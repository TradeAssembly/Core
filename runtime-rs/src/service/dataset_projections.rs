// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Owned capture projection, not acquisition, provider authentication or execution.
//! Publication is a retained FSM across independent repository/KV/ownership ports.
use super::{data_captures, data_plans::PlanScope, ServiceResponse, TradeAssemblyService};
use crate::{data_pipeline::canonical_sha256, historical_data::*, ports::*};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tradeassembly_plugin_sdk::{data_evaluation::*, market_data};

pub(super) const CREATE: &str = "/dataset-projections";
const STATES: &str = "dataset_capture_projection_states_v1";
const PROOFS: &str = "dataset_capture_projection_receipts_v1";
const LINK: &str = "captureProjectionId";
const DAY: i64 = 86400000;
const LIMIT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Version {
    #[serde(rename = "tradeassembly.dataset_capture_projection.v1")]
    V1,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Profile {
    #[serde(rename = "crypto_spot_utc_daily_diagnostic_v1")]
    Diagnostic,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimestampBasis {
    EventStart,
    EventEnd,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturePin {
    pub capture_id: String,
    pub receipt_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectionCommand {
    pub schema_version: Version,
    /// Stable caller key for this owner. Recovery uses the same key with a NEW
    /// outer C3 idempotency key; an interrupted C3 command is never redispatched.
    pub projection_key: String,
    pub scope: PlanScope,
    pub capture: CapturePin,
    pub replay_output_port: String,
    pub profile: Profile,
    pub timestamp_basis: TimestampBasis,
    pub as_of_ms: i64,
    pub acknowledge_modeled_calendar: bool,
    pub acknowledge_retrospective_availability: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdvanceCommand {
    schema_version: Version,
    projection_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    SnapshotStored,
    ProofStored,
    Published,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Evidence {
    schema_version: String,
    owner_sha256: String,
    request_sha256: String,
    scope: PlanScope,
    capture_id: String,
    capture_receipt_sha256: String,
    acquisition_binding_sha256: String,
    replay_binding_sha256: String,
    replay_contract_sha256: String,
    replay_output_port: String,
    replay_output_contract: tradeassembly_plugin_sdk::data_contract::DataPort,
    input_witness: data_captures::InputWitness,
    acquisition_observed_at_ms: i64,
    replay_as_of_ms: i64,
    replay_observed_at_ms: i64,
    as_of_ms: i64,
    timestamp_basis: TimestampBasis,
    modeled_calendar: String,
    provider_authenticity_established: bool,
    points: Vec<DataPoint>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Proof {
    schema_version: String,
    projection_id: String,
    evidence: Evidence,
    dataset_id: String,
    content_hash: String,
    proof_sha256: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    schema_version: String,
    projection_id: String,
    owner_sha256: String,
    command: ProjectionCommand,
    request_sha256: String,
    snapshot: DatasetSnapshot,
    proof: Proof,
    phase: Phase,
    state_sha256: String,
}
fn hash<T: Serialize>(v: &T) -> Result<String, String> {
    canonical_sha256(v).map_err(|_| "dataset_projection_encoding_invalid".into())
}
fn value<T: Serialize>(v: &T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|_| "dataset_projection_encoding_invalid".into())
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, String> {
    if serde_json::to_vec(&v)
        .map_err(|_| "dataset_projection_encoding_invalid")?
        .len()
        > LIMIT
    {
        return Err("dataset_projection_budget_exceeded".into());
    }
    serde_json::from_value(v).map_err(|_| "dataset_projection_record_invalid".into())
}
fn label(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
fn digest(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn owner(s: &TradeAssemblyService) -> Result<String, String> {
    hash(&super::typed_backtest::actual_owner(s)?)
}
fn scope(s: &TradeAssemblyService, c: &ProjectionCommand) -> Result<(), String> {
    owner(s)?;
    s.require_object("strategy", &c.scope.strategy_id)
        .map_err(|_| "object_not_available")?;
    if !label(&c.projection_key)
        || !label(&c.replay_output_port)
        || !digest(&c.capture.receipt_sha256)
        || !c.acknowledge_modeled_calendar
        || !c.acknowledge_retrospective_availability
        || c.as_of_ms <= 0
        || c.as_of_ms > s.runtime().clock.trusted_now_ms()?
    {
        return Err("dataset_projection_command_invalid".into());
    }
    Ok(())
}
fn id(s: &TradeAssemblyService, c: &ProjectionCommand) -> Result<String, String> {
    Ok(format!(
        "projection_{}",
        hash(&json!([owner(s)?, c.projection_key]))?
    ))
}
fn seal_state(v: &mut State) -> Result<(), String> {
    v.state_sha256.clear();
    v.state_sha256 = hash(v)?;
    Ok(())
}
fn seal_proof(v: &mut Proof) -> Result<(), String> {
    v.proof_sha256.clear();
    v.proof_sha256 = hash(v)?;
    Ok(())
}
fn utc(ms: i64) -> Result<String, String> {
    chrono::DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|v| v.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or("dataset_projection_time_invalid".into())
}
fn exact_micros(v: f64) -> Result<(), String> {
    use tradeassembly_plugin_sdk::execution::{Decimal, Rounding};
    if !v.is_finite() || v < 0.0 {
        return Err("dataset_projection_numeric_invalid".into());
    }
    let n: Decimal = v.to_string().parse()?;
    let _: i64 = n
        .multiply("1000000".parse()?)?
        .round(0, Rounding::Exact)?
        .to_string()
        .parse()
        .map_err(|_| "dataset_projection_precision_unsupported")?;
    Ok(())
}
fn capture(
    s: &TradeAssemblyService,
    c: &ProjectionCommand,
) -> Result<data_captures::CaptureReceipt, String> {
    scope(s, c)?;
    let r = data_captures::current(s, &c.capture.capture_id)?;
    data_captures::verified_input_witness(&r)?;
    if r.receipt_sha256 != c.capture.receipt_sha256
        || r.policy.scope != c.scope
        || r.policy.mode != "backtest"
    {
        return Err("dataset_projection_capture_mismatch".into());
    }
    if r.acquisition_response.observed_at_ms > c.as_of_ms
        || r.replay_observed_at_ms > c.as_of_ms
        || r.replay_as_of_ms > c.as_of_ms
    {
        return Err("dataset_projection_future_availability".into());
    }
    Ok(r)
}
fn build(
    s: &TradeAssemblyService,
    c: &ProjectionCommand,
    created: i64,
) -> Result<(DatasetSnapshot, Proof), String> {
    let r = capture(s, c)?;
    let p = &r.policy;
    let [instrument] = p.context.instruments.as_slice() else {
        return Err("dataset_projection_profile_unsupported".into());
    };
    let count = p
        .event_end_ms
        .checked_sub(p.event_start_ms)
        .filter(|n| *n > 0)
        .ok_or("dataset_projection_profile_unsupported")?
        / DAY;
    if p.context.interval_ms != Some(DAY as u64)
        || instrument.asset_class != "crypto"
        || instrument.instrument_family != "crypto_spot"
        || p.event_start_ms <= 0
        || p.event_start_ms % DAY != 0
        || p.event_end_ms % DAY != 0
        || !(1..=256).contains(&count)
    {
        return Err("dataset_projection_profile_unsupported".into());
    }
    let contract = s
        .runtime()
        .plugin_packages
        .operation_data(&r.replay.package_sha256, &r.replay.operation_id)?
        .ok_or("dataset_projection_contract_missing")?;
    let port = contract
        .outputs
        .get(&c.replay_output_port)
        .ok_or("dataset_projection_output_missing")?;
    if *port != market_data::ohlcv_output_port(DAY as u64)?
        || hash(&contract)? != r.replay_contract_sha256
    {
        return Err("dataset_projection_ohlcv_identity_required".into());
    }
    let batch = r
        .replay_response
        .outputs
        .get(&c.replay_output_port)
        .ok_or("dataset_projection_output_missing")?;
    if batch.schema != port.schema
        || batch.shape != port.shape
        || batch.points.len() != count as usize
    {
        return Err("dataset_projection_coverage_invalid".into());
    }
    let mut rows = Vec::with_capacity(batch.points.len());
    for (index, point) in batch.points.iter().enumerate() {
        let start = p.event_start_ms + index as i64 * DAY;
        let prov = &point.provenance;
        if point.instrument_id != instrument.instrument_id
            || !prov.completed
            || prov.event_start_ms != start
            || prov.event_end_ms != start + DAY
            || prov.known_at_ms < prov.event_end_ms
            || prov.arrived_at_ms < prov.known_at_ms
            || prov.known_at_ms > c.as_of_ms
            || prov.arrived_at_ms > c.as_of_ms
            || point.source_event_id.is_empty()
            || prov.source_revision.is_empty()
            || point_sha256(point).map_err(|_| "dataset_projection_point_invalid")?
                != prov.content_sha256
        {
            return Err("dataset_projection_coverage_or_availability_invalid".into());
        }
        let PointValue::Ready(raw) = &point.result else {
            return Err("dataset_projection_ready_series_required".into());
        };
        let bar: market_data::OhlcvBar =
            serde_json::from_value(raw.clone()).map_err(|_| "dataset_projection_bar_invalid")?;
        bar.validate()?;
        for v in [bar.open, bar.high, bar.low, bar.close, bar.volume]
            .into_iter()
            .chain(bar.vwap)
        {
            exact_micros(v)?;
        }
        let session = bar
            .session
            .as_ref()
            .map(|session| {
                if session.calendar != "crypto_24x7"
                    || session.start_ms > start
                    || session.end_ms < start + DAY
                {
                    return Err("dataset_projection_calendar_contradiction".to_string());
                }
                Ok(BarSession {
                    calendar: session.calendar.clone(),
                    start: utc(session.start_ms)?,
                    end: utc(session.end_ms)?,
                    source_ref: session.source_ref.clone(),
                })
            })
            .transpose()?;
        rows.push(HistoricalObservation {
            instrument_id: point.instrument_id.clone(),
            timestamp: utc(match c.timestamp_basis {
                TimestampBasis::EventStart => start,
                TimestampBasis::EventEnd => start + DAY,
            })?,
            data: HistoricalObservationData::Bar(BarObservation {
                open: bar.open,
                high: bar.high,
                low: bar.low,
                close: bar.close,
                volume: bar.volume,
                vwap: bar.vwap,
                session,
            }),
        });
    }
    let projection_id = id(s, c)?;
    let evidence = Evidence {
        schema_version: "tradeassembly.ohlcv_capture_projection_evidence.v1".into(),
        owner_sha256: owner(s)?,
        request_sha256: hash(c)?,
        scope: c.scope.clone(),
        capture_id: r.capture_id.clone(),
        capture_receipt_sha256: r.receipt_sha256.clone(),
        acquisition_binding_sha256: hash(&r.acquisition)?,
        replay_binding_sha256: hash(&r.replay)?,
        replay_contract_sha256: r.replay_contract_sha256.clone(),
        replay_output_port: c.replay_output_port.clone(),
        replay_output_contract: port.clone(),
        input_witness: data_captures::verified_input_witness(&r)?.clone(),
        acquisition_observed_at_ms: r.acquisition_response.observed_at_ms,
        replay_as_of_ms: r.replay_as_of_ms,
        replay_observed_at_ms: r.replay_observed_at_ms,
        as_of_ms: c.as_of_ms,
        timestamp_basis: c.timestamp_basis.clone(),
        modeled_calendar: "explicit_research_model_crypto_24x7_UTC_daily".into(),
        provider_authenticity_established: false,
        points: batch.points.clone(),
    };
    // Bind a pre-dataset evidence digest to avoid a circular dataset/proof hash.
    let upstream = BTreeMap::from([
        (LINK.into(), projection_id.clone()),
        ("captureId".into(), r.capture_id.clone()),
        ("captureReceiptSha256".into(), r.receipt_sha256.clone()),
        ("projectionEvidenceSha256".into(), hash(&evidence)?),
        (
            "availabilitySemantics".into(),
            "retrospective_explicit_as_of_no_point_in_time_claim".into(),
        ),
    ]);
    let graph = s
        .runtime()
        .storage
        .get_json(
            super::capability_graph::REVISIONS_NS,
            &p.capability_graph_revision,
        )?
        .ok_or("dataset_projection_graph_missing")?;
    let graph_fingerprint = graph["graphFingerprint"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or("dataset_projection_graph_invalid")?
        .to_owned();
    let reject = QualityDisposition::Reject;
    let snapshot = DatasetSnapshot::build(
        DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.into(),
            strategy_id: c.scope.strategy_id.clone(),
            strategy_version_id: Some(c.scope.strategy_version_id.clone()),
            source: DatasetSourceManifest {
                source_class: DatasetSourceClass::Test,
                plugin_instance_ref: r.acquisition.plugin_instance_ref,
                plugin_ref: r.acquisition.plugin_ref,
                operation_id: r.acquisition.operation_id,
                plugin_manifest_fingerprint: r.acquisition.manifest_sha256,
                capability_graph_revision_id: p.capability_graph_revision.clone(),
                capability_graph_fingerprint: graph_fingerprint,
                source_request_hash: r.input_sha256,
                upstream_refs: upstream,
            },
            asset_classes: vec![AssetClass::CryptoSpot],
            instruments: vec![instrument.instrument_id.clone()],
            data_kind: HistoricalDataKind::Bars,
            granularity: "1d".into(),
            time_slice: DatasetTimeSlice {
                start: utc(p.event_start_ms)?,
                end: utc(p.event_end_ms)?,
            },
            calendar: "crypto_24x7".into(),
            timezone: "UTC".into(),
            normalization_policy: DatasetNormalizationPolicy {
                timestamp_unit: "rfc3339".into(),
                timezone: "UTC".into(),
                duplicate_policy: "reject".into(),
                price_adjustment: "none".into(),
            },
            quality_policy: DatasetQualityPolicy {
                missing_intervals: reject.clone(),
                stale_observations: reject.clone(),
                outliers: reject.clone(),
                invalid_markets: reject.clone(),
                calendar_mismatch: reject.clone(),
                corporate_action_gaps: reject.clone(),
                missing_derivative_fields: reject,
            },
            quality_findings: vec![],
            observations: rows,
        },
        created,
    )?;
    crate::backtest_engine::validate_typed_precision(&snapshot)?;
    let mut proof = Proof {
        schema_version: "tradeassembly.dataset_capture_projection_receipt.v1".into(),
        projection_id,
        evidence,
        dataset_id: snapshot.dataset_id.clone(),
        content_hash: snapshot.content_hash.clone(),
        proof_sha256: String::new(),
    };
    seal_proof(&mut proof)?;
    Ok((snapshot, proof))
}
fn load(s: &TradeAssemblyService, projection: &str) -> Result<(Value, State), String> {
    if !projection.strip_prefix("projection_").is_some_and(digest) {
        return Err("object_not_available".into());
    }
    let raw = s
        .runtime()
        .storage
        .get_json(STATES, projection)?
        .ok_or("object_not_available")?;
    let record: State = decode(raw.clone())?;
    if record.owner_sha256 != owner(s)? {
        return Err("object_not_available".into());
    }
    s.require_object("strategy", &record.command.scope.strategy_id)
        .map_err(|_| "object_not_available")?;
    let mut checked = record.clone();
    seal_state(&mut checked)?;
    let mut proof = record.proof.clone();
    seal_proof(&mut proof)?;
    record.snapshot.verify()?;
    if checked != record
        || proof != record.proof
        || record.schema_version != "tradeassembly.dataset_capture_projection_state.v1"
        || record.proof.schema_version != "tradeassembly.dataset_capture_projection_receipt.v1"
        || record.projection_id != projection
        || id(s, &record.command)? != projection
        || record.request_sha256 != hash(&record.command)?
        || record.proof.evidence.request_sha256 != record.request_sha256
        || record.proof.evidence.scope != record.command.scope
        || record.proof.evidence.capture_id != record.command.capture.capture_id
        || record.proof.evidence.capture_receipt_sha256 != record.command.capture.receipt_sha256
        || record.proof.evidence.owner_sha256 != record.owner_sha256
        || record.proof.projection_id != record.projection_id
        || record.proof.dataset_id != record.snapshot.dataset_id
        || record.proof.content_hash != record.snapshot.content_hash
        || record.snapshot.content.source.upstream_refs.get(LINK) != Some(&record.projection_id)
        || record
            .snapshot
            .content
            .source
            .upstream_refs
            .get("projectionEvidenceSha256")
            != Some(&hash(&record.proof.evidence)?)
    {
        return Err("dataset_projection_record_invalid".into());
    }
    Ok((raw, record))
}
fn stored_proof(s: &TradeAssemblyService, r: &State) -> Result<(), String> {
    let raw = s
        .runtime()
        .storage
        .get_json(PROOFS, &r.projection_id)?
        .ok_or("dataset_projection_proof_missing")?;
    let proof: Proof = decode(raw)?;
    if proof != r.proof {
        return Err("dataset_projection_proof_invalid".into());
    }
    let snapshot = s
        .runtime()
        .dataset_snapshots
        .get(&r.snapshot.dataset_id)?
        .ok_or("dataset_projection_snapshot_missing")?;
    snapshot.verify()?;
    if snapshot != r.snapshot {
        return Err("dataset_projection_snapshot_mismatch".into());
    }
    Ok(())
}
fn published(s: &TradeAssemblyService, r: &State) -> Result<(), String> {
    if r.phase != Phase::Published {
        return Err("dataset_projection_not_published".into());
    }
    stored_proof(s, r)?;
    let binding = s
        .runtime()
        .object_authorization
        .scope("dataset_snapshot", &r.snapshot.dataset_id)?
        .ok_or("dataset_projection_not_published")?;
    if binding.owner != super::typed_backtest::actual_owner(s)?
        || binding.parent_type.as_deref() != Some("strategy")
        || binding.parent_id.as_deref() != Some(r.command.scope.strategy_id.as_str())
    {
        return Err("object_not_available".into());
    }
    s.require_object("dataset_snapshot", &r.snapshot.dataset_id)
        .map_err(|_| "object_not_available".into())
}
/// Historical integrity/ownership only, distinct from creating or consuming new research.
pub(super) fn verify_publication(
    s: &TradeAssemblyService,
    snapshot: &DatasetSnapshot,
) -> Result<(), String> {
    let Some(projection) = snapshot.content.source.upstream_refs.get(LINK) else {
        return Ok(());
    };
    let (_, r) = load(s, projection)?;
    published(s, &r)?;
    if r.snapshot != *snapshot {
        return Err("dataset_projection_snapshot_mismatch".into());
    }
    Ok(())
}
/// Workspace snapshots intentionally omit raw observations. Validate their
/// exact historical metadata against the owned, fully published retained proof
/// rather than deserializing that redacted row as a complete dataset.
pub(super) fn verify_publication_metadata(
    s: &TradeAssemblyService,
    metadata: &Value,
) -> Result<(), String> {
    let projection = metadata["content"]["source"]["upstreamRefs"][LINK]
        .as_str()
        .ok_or("dataset_projection_reference_invalid")?;
    let (_, r) = load(s, projection)?;
    published(s, &r)?;
    let mut expected = value(&r.snapshot)?;
    expected["content"]
        .as_object_mut()
        .ok_or("dataset_projection_reference_invalid")?
        .remove("observations");
    if expected != *metadata {
        return Err("dataset_projection_reference_mismatch".into());
    }
    Ok(())
}
/// Narrow admission hook for datasets created by this new contract. Legacy
/// datasets retain their original behavior. A hash/link alone is not source proof.
fn verified_research_projection(
    s: &TradeAssemblyService,
    snapshot: &DatasetSnapshot,
) -> Result<Option<State>, String> {
    let Some(projection) = snapshot.content.source.upstream_refs.get(LINK) else {
        return Ok(None);
    };
    let (_, r) = load(s, projection)?;
    published(s, &r)?;
    if r.snapshot != *snapshot {
        return Err("dataset_projection_snapshot_mismatch".into());
    }
    let (fresh, proof) = build(s, &r.command, r.snapshot.created_at_ms)?;
    if fresh != r.snapshot || proof != r.proof {
        return Err("dataset_projection_source_changed".into());
    }
    Ok(Some(r))
}
pub(super) fn verify_for_research(
    s: &TradeAssemblyService,
    snapshot: &DatasetSnapshot,
) -> Result<(), String> {
    verified_research_projection(s, snapshot).map(|_| ())
}
/// Fill evidence must also have existed by the consuming run's explicit cutoff,
/// which can be earlier than the projection's own acknowledged cutoff.
pub(super) fn verify_for_research_at(
    s: &TradeAssemblyService,
    snapshot: &DatasetSnapshot,
    as_of_ms: i64,
) -> Result<(), String> {
    let Some(r) = verified_research_projection(s, snapshot)? else {
        return Ok(());
    };
    let e = &r.proof.evidence;
    if as_of_ms <= 0
        || e.acquisition_observed_at_ms > as_of_ms
        || e.replay_as_of_ms > as_of_ms
        || e.replay_observed_at_ms > as_of_ms
        || e.points
            .iter()
            .any(|p| p.provenance.known_at_ms > as_of_ms || p.provenance.arrived_at_ms > as_of_ms)
    {
        return Err("dataset_projection_consumer_cutoff_unavailable".into());
    }
    Ok(())
}

fn response(s: &TradeAssemblyService, r: &State) -> Result<ServiceResponse, String> {
    let ready = r.phase == Phase::Published;
    if ready {
        published(s, r)?;
    }
    Ok(ServiceResponse {
        status: if ready { 200 } else { 202 },
        body: json!({"schemaVersion":"tradeassembly.dataset_capture_projection_result.v1","projectionId":r.projection_id,"phase":r.phase,"published":ready,"dataset":if ready{json!({"datasetId":r.snapshot.dataset_id,"snapshotRef":r.snapshot.snapshot_ref,"contentHash":r.snapshot.content_hash,"rowCount":r.snapshot.row_count,"sourceClass":"test"})}else{Value::Null},"proofSha256":if ready{Some(&r.proof.proof_sha256)}else{None},"providerAuthenticityEstablished":false,"retrospectiveAvailability":true,"historicalOutcome":true,"executionAuthorityGranted":false}),
    })
}
fn create_inner(
    s: &TradeAssemblyService,
    c: ProjectionCommand,
    ctx: &SideEffectContext,
) -> Result<ServiceResponse, String> {
    scope(s, &c)?;
    let projection = id(s, &c)?;
    if s.runtime().storage.get_json(STATES, &projection)?.is_some() {
        let (_, r) = load(s, &projection)?;
        if r.command != c {
            return Err("dataset_projection_idempotency_conflict".into());
        }
        return response(s, &r);
    }
    let (snapshot, proof) = build(s, &c, s.runtime().clock.trusted_now_ms()?)?;
    let mut r = State {
        schema_version: "tradeassembly.dataset_capture_projection_state.v1".into(),
        projection_id: projection.clone(),
        owner_sha256: owner(s)?,
        request_sha256: hash(&c)?,
        command: c,
        snapshot,
        proof,
        phase: Phase::Prepared,
        state_sha256: String::new(),
    };
    seal_state(&mut r)?;
    let encoded = value(&r)?;
    let _: State = decode(encoded.clone())?;
    s.runtime()
        .storage
        .put_json_if_absent(STATES, &projection, encoded, ctx)?;
    let (_, saved) = load(s, &projection)?;
    if saved.command != r.command || saved.request_sha256 != r.request_sha256 {
        return Err("dataset_projection_idempotency_conflict".into());
    }
    response(s, &saved)
}
fn advance_inner(
    s: &TradeAssemblyService,
    projection: &str,
    ctx: &SideEffectContext,
) -> Result<ServiceResponse, String> {
    for _ in 0..4 {
        let (old, mut r) = load(s, projection)?;
        if r.phase == Phase::Published {
            return response(s, &r);
        }
        let (fresh, proof) = build(s, &r.command, r.snapshot.created_at_ms)?;
        if fresh != r.snapshot || proof != r.proof {
            return Err("dataset_projection_source_changed".into());
        }
        r.phase = match r.phase {
            Phase::Prepared => {
                s.runtime().dataset_snapshots.put(&r.snapshot, ctx)?;
                Phase::SnapshotStored
            }
            Phase::SnapshotStored => {
                s.runtime().storage.put_json_if_absent(
                    PROOFS,
                    projection,
                    value(&r.proof)?,
                    ctx,
                )?;
                stored_proof(s, &r)?;
                Phase::ProofStored
            }
            Phase::ProofStored => {
                stored_proof(s, &r)?;
                // This ownership write is the publication boundary. Source and
                // exact owner were just rechecked; proof is durable BEFORE it.
                s.bind_inherited_object(
                    "dataset_snapshot",
                    &r.snapshot.dataset_id,
                    "strategy",
                    &r.command.scope.strategy_id,
                )?;
                Phase::Published
            }
            Phase::Published => unreachable!(),
        };
        seal_state(&mut r)?;
        if s.runtime()
            .storage
            .compare_and_put_json(STATES, projection, old, value(&r)?, ctx)?
            == ComparePutOutcome::Updated
            && r.phase == Phase::Published
        {
            return response(s, &r);
        }
    }
    let (_, r) = load(s, projection)?;
    response(s, &r)
}
fn context(
    s: &TradeAssemblyService,
    path: &str,
    body: &Value,
) -> Result<SideEffectContext, String> {
    let e = crate::control_plane::http_envelope("http", "POST", path, body)?;
    Ok(SideEffectContext::new(
        e.authority,
        IdempotencyKey::new(
            body["idempotencyKey"]
                .as_str()
                .ok_or("dataset_projection_outer_key_required")?,
        )?,
    )
    .with_agent_execution(s.agent_mcp_execution_context()))
}
fn error(code: String) -> ServiceResponse {
    if code == "object_not_available" {
        ServiceResponse::object_unavailable()
    } else {
        ServiceResponse::conflict(&code)
    }
}
fn request(body: &Value) -> Result<ProjectionCommand, String> {
    decode(body["projection"].clone()).map_err(|_| "dataset_projection_command_invalid".into())
}
fn advance_request(path: &str, body: &Value) -> Result<AdvanceCommand, String> {
    let c: AdvanceCommand = decode(body["projection"].clone())?;
    if path != format!("{CREATE}/{}/advance", c.projection_id) {
        return Err("dataset_projection_path_mismatch".into());
    }
    Ok(c)
}
pub(super) fn create(s: &TradeAssemblyService, body: Value) -> ServiceResponse {
    (|| {
        let c = request(&body)?;
        create_inner(s, c, &context(s, CREATE, &body)?)
    })()
    .unwrap_or_else(error)
}
pub(super) fn advance(s: &TradeAssemblyService, path: &str, body: Value) -> ServiceResponse {
    (|| {
        let c = advance_request(path, &body)?;
        advance_inner(s, &c.projection_id, &context(s, path, &body)?)
    })()
    .unwrap_or_else(error)
}
pub(super) fn read(s: &TradeAssemblyService, projection: &str) -> ServiceResponse {
    (|| {
        let (_, r) = load(s, projection)?;
        let mut response = response(s, &r)?;
        if r.phase == Phase::Published {
            response.body["projectionReceipt"] = value(&r.proof)?;
        }
        Ok(response)
    })()
    .unwrap_or_else(error)
}
/// Before generic C3 replay/prepare, owner and retained request are checked now.
pub(super) fn preflight(
    s: &TradeAssemblyService,
    method: &str,
    path: &str,
    body: &Value,
) -> Option<ServiceResponse> {
    let relevant = path == CREATE || path.starts_with(&format!("{CREATE}/"));
    if !relevant {
        return None;
    }
    let result: Result<(), String> = (|| {
        if method == "POST" {
            context(s, path, body)?;
            if path == CREATE {
                let c = request(body)?;
                scope(s, &c)?;
                let projection = id(s, &c)?;
                if s.runtime().storage.get_json(STATES, &projection)?.is_some() {
                    let (_, r) = load(s, &projection)?;
                    if r.command != c {
                        return Err("dataset_projection_idempotency_conflict".into());
                    }
                    if r.phase == Phase::Published {
                        published(s, &r)?;
                    }
                } else {
                    build(s, &c, s.runtime().clock.trusted_now_ms()?)?;
                }
            } else {
                let c = advance_request(path, body)?;
                let (_, r) = load(s, &c.projection_id)?;
                if r.phase == Phase::Published {
                    published(s, &r)?
                } else {
                    capture(s, &r.command)?;
                }
            }
        } else if method == "GET" {
            load(s, path.trim_start_matches(&format!("{CREATE}/")))?;
        } else {
            return Err("dataset_projection_method_unsupported".into());
        }
        Ok(())
    })();
    result.err().map(error)
}
#[cfg(test)]
#[path = "dataset_projections_tests.rs"]
mod tests;
