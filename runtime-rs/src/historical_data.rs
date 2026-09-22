// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const DATASET_SNAPSHOT_SCHEMA: &str = "tradeassembly.dataset-snapshot.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    Equity,
    Etf,
    CryptoSpot,
    Option,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoricalDataKind {
    Bars,
    OptionChain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetSourceClass {
    Production,
    Test,
    Demo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetIngestionState {
    Requested,
    ResolvingSource,
    Ingesting,
    Validating,
    Completed,
    Blocked,
    Failed,
    Canceled,
}

impl DatasetIngestionState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Blocked | Self::Failed | Self::Canceled
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetTimeSlice {
    pub start: String,
    pub end: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityDisposition {
    Reject,
    Warn,
    Allow,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetQualityPolicy {
    pub missing_intervals: QualityDisposition,
    pub stale_observations: QualityDisposition,
    pub outliers: QualityDisposition,
    pub invalid_markets: QualityDisposition,
    pub calendar_mismatch: QualityDisposition,
    pub corporate_action_gaps: QualityDisposition,
    pub missing_derivative_fields: QualityDisposition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetNormalizationPolicy {
    pub timestamp_unit: String,
    pub timezone: String,
    pub duplicate_policy: String,
    pub price_adjustment: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum HistoricalObservationData {
    Bar(BarObservation),
    OptionContract(Box<OptionContractObservation>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BarObservation {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    /// Provider-reported volume-weighted price for this bar; never inferred from OHLC.
    /// Omission preserves the serialized identity of existing snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vwap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<BarSession>,
}

/// Provider calendar evidence for the bar's trading date, including pre/post-market bars.
/// Session membership is evaluated separately; evidence is part of dataset identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BarSession {
    pub calendar: String,
    pub start: String,
    pub end: String,
    pub source_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionRight {
    Call,
    Put,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionGreeks {
    pub delta: Option<f64>,
    pub gamma: Option<f64>,
    pub theta: Option<f64>,
    pub vega: Option<f64>,
    pub rho: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OptionContractObservation {
    pub contract_symbol: String,
    pub underlying_instrument_id: String,
    pub expiration: String,
    pub strike: f64,
    pub right: OptionRight,
    pub bid: Option<f64>,
    pub ask: Option<f64>,
    pub last: Option<f64>,
    pub volume: Option<f64>,
    pub open_interest: Option<f64>,
    pub implied_volatility: Option<f64>,
    pub greeks: Option<OptionGreeks>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoricalObservation {
    pub instrument_id: String,
    pub timestamp: String,
    pub data: HistoricalObservationData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetQualitySeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetQualityFinding {
    pub code: String,
    pub severity: DatasetQualitySeverity,
    pub instrument_id: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetSourceManifest {
    pub source_class: DatasetSourceClass,
    pub plugin_instance_ref: String,
    pub plugin_ref: String,
    pub operation_id: String,
    pub plugin_manifest_fingerprint: String,
    pub capability_graph_revision_id: String,
    pub capability_graph_fingerprint: String,
    pub source_request_hash: String,
    #[serde(default)]
    pub upstream_refs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetSnapshotContent {
    pub schema: String,
    pub strategy_id: String,
    pub strategy_version_id: Option<String>,
    pub source: DatasetSourceManifest,
    pub asset_classes: Vec<AssetClass>,
    pub instruments: Vec<String>,
    pub data_kind: HistoricalDataKind,
    pub granularity: String,
    pub time_slice: DatasetTimeSlice,
    pub calendar: String,
    pub timezone: String,
    pub normalization_policy: DatasetNormalizationPolicy,
    pub quality_policy: DatasetQualityPolicy,
    #[serde(default)]
    pub quality_findings: Vec<DatasetQualityFinding>,
    pub observations: Vec<HistoricalObservation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetSnapshot {
    pub dataset_id: String,
    pub snapshot_ref: String,
    pub content_hash: String,
    pub byte_length: u64,
    pub row_count: u64,
    pub first_timestamp: String,
    pub last_timestamp: String,
    pub created_at_ms: i64,
    pub content: DatasetSnapshotContent,
}

impl DatasetSnapshot {
    pub fn build(content: DatasetSnapshotContent, created_at_ms: i64) -> Result<Self, String> {
        validate_content(&content)?;
        let bytes = canonical_content_bytes(&content)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let dataset_id = format!("dataset_{digest}");
        let first_timestamp = content
            .observations
            .iter()
            .map(|row| row.timestamp.as_str())
            .min()
            .map(str::to_string)
            .ok_or_else(|| "dataset_snapshot_rows_required".to_string())?;
        let last_timestamp = content
            .observations
            .iter()
            .map(|row| row.timestamp.as_str())
            .max()
            .map(str::to_string)
            .ok_or_else(|| "dataset_snapshot_rows_required".to_string())?;
        Ok(Self {
            snapshot_ref: format!("tradeassembly://datasets/{dataset_id}"),
            content_hash: format!("sha256:{digest}"),
            byte_length: bytes.len() as u64,
            row_count: content.observations.len() as u64,
            dataset_id,
            first_timestamp,
            last_timestamp,
            created_at_ms,
            content,
        })
    }

    pub fn verify(&self) -> Result<(), String> {
        let rebuilt = Self::build(self.content.clone(), self.created_at_ms)
            .map_err(|_| "dataset_snapshot_integrity_failed".to_string())?;
        if self.dataset_id != rebuilt.dataset_id
            || self.snapshot_ref != rebuilt.snapshot_ref
            || self.content_hash != rebuilt.content_hash
            || self.byte_length != rebuilt.byte_length
            || self.row_count != rebuilt.row_count
            || self.first_timestamp != rebuilt.first_timestamp
            || self.last_timestamp != rebuilt.last_timestamp
        {
            return Err("dataset_snapshot_integrity_failed".to_string());
        }
        Ok(())
    }

    pub fn canonical_content_bytes(&self) -> Result<Vec<u8>, String> {
        canonical_content_bytes(&self.content)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatasetIngestion {
    pub ingestion_id: String,
    pub idempotency_key: String,
    pub request_hash: String,
    pub strategy_id: String,
    pub strategy_version_id: Option<String>,
    pub state: DatasetIngestionState,
    pub attempt: u32,
    pub fencing_token: i64,
    pub requested_at_ms: i64,
    pub updated_at_ms: i64,
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub diagnostics: Vec<DatasetQualityFinding>,
}

fn canonical_content_bytes(content: &DatasetSnapshotContent) -> Result<Vec<u8>, String> {
    serde_json_canonicalizer::to_vec(content)
        .map_err(|_| "dataset_snapshot_serialization_failed".to_string())
}

fn validate_content(content: &DatasetSnapshotContent) -> Result<(), String> {
    if content.schema != DATASET_SNAPSHOT_SCHEMA
        || content.strategy_id.trim().is_empty()
        || content.source.plugin_instance_ref.trim().is_empty()
        || content.source.plugin_ref.trim().is_empty()
        || content.source.operation_id.trim().is_empty()
        || content.source.plugin_manifest_fingerprint.trim().is_empty()
        || content
            .source
            .capability_graph_revision_id
            .trim()
            .is_empty()
        || content
            .source
            .capability_graph_fingerprint
            .trim()
            .is_empty()
        || content.source.source_request_hash.trim().is_empty()
        || content.instruments.is_empty()
        || content.granularity.trim().is_empty()
        || content.time_slice.start.trim().is_empty()
        || content.time_slice.end.trim().is_empty()
        || content.time_slice.start >= content.time_slice.end
        || content.calendar.trim().is_empty()
        || content.timezone.trim().is_empty()
        || content.observations.is_empty()
    {
        return Err("dataset_snapshot_content_invalid".to_string());
    }
    let mut previous: Option<(&str, &str)> = None;
    for observation in &content.observations {
        if observation.instrument_id.trim().is_empty()
            || observation.timestamp.trim().is_empty()
            || observation.timestamp < content.time_slice.start
            || observation.timestamp > content.time_slice.end
        {
            return Err("dataset_snapshot_row_invalid".to_string());
        }
        validate_observation(content, observation)?;
        let current = (
            observation.instrument_id.as_str(),
            observation.timestamp.as_str(),
        );
        if previous.is_some_and(|prior| prior >= current) {
            return Err("dataset_snapshot_rows_unordered".to_string());
        }
        previous = Some(current);
    }
    Ok(())
}

fn validate_observation(
    content: &DatasetSnapshotContent,
    observation: &HistoricalObservation,
) -> Result<(), String> {
    match (&content.data_kind, &observation.data) {
        (HistoricalDataKind::Bars, HistoricalObservationData::Bar(bar)) => {
            if let Some(session) = &bar.session {
                let start = chrono::DateTime::parse_from_rfc3339(&session.start)
                    .map_err(|_| "dataset_snapshot_session_invalid".to_string())?;
                let end = chrono::DateTime::parse_from_rfc3339(&session.end)
                    .map_err(|_| "dataset_snapshot_session_invalid".to_string())?;
                if session.calendar != content.calendar
                    || session.source_ref.trim().is_empty()
                    || start >= end
                {
                    return Err("dataset_snapshot_session_invalid".to_string());
                }
            }
            if !content.instruments.contains(&observation.instrument_id)
                || ![bar.open, bar.high, bar.low, bar.close, bar.volume]
                    .iter()
                    .all(|value| value.is_finite())
                || bar.open < 0.0
                || bar.high < 0.0
                || bar.low < 0.0
                || bar.close < 0.0
                || bar.volume < 0.0
                || bar.vwap.is_some_and(|value| {
                    !value.is_finite() || value <= 0.0 || value < bar.low || value > bar.high
                })
                || bar.high < bar.open.max(bar.close)
                || bar.low > bar.open.min(bar.close)
                || bar.low > bar.high
            {
                return Err("dataset_snapshot_bar_invalid".to_string());
            }
        }
        (HistoricalDataKind::OptionChain, HistoricalObservationData::OptionContract(option)) => {
            if option.contract_symbol.trim().is_empty()
                || observation.instrument_id != option.contract_symbol
                || option.underlying_instrument_id.trim().is_empty()
                || !content
                    .instruments
                    .contains(&option.underlying_instrument_id)
                || option.expiration.trim().is_empty()
                || !option.strike.is_finite()
                || option.strike <= 0.0
                || [option.bid, option.ask, option.last]
                    .into_iter()
                    .flatten()
                    .next()
                    .is_none()
                || option_values(option).any(|value| !value.is_finite() || value < 0.0)
                || matches!((option.bid, option.ask), (Some(bid), Some(ask)) if bid > ask)
                || option
                    .greeks
                    .as_ref()
                    .is_some_and(|greeks| greeks_values(greeks).any(|value| !value.is_finite()))
            {
                return Err("dataset_snapshot_option_invalid".to_string());
            }
        }
        _ => return Err("dataset_snapshot_schema_mismatch".to_string()),
    }
    Ok(())
}

fn option_values(option: &OptionContractObservation) -> impl Iterator<Item = f64> + '_ {
    [
        option.bid,
        option.ask,
        option.last,
        option.volume,
        option.open_interest,
        option.implied_volatility,
    ]
    .into_iter()
    .flatten()
}

fn greeks_values(greeks: &OptionGreeks) -> impl Iterator<Item = f64> + '_ {
    [
        greeks.delta,
        greeks.gamma,
        greeks.theta,
        greeks.vega,
        greeks.rho,
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> DatasetSnapshotContent {
        DatasetSnapshotContent {
            schema: DATASET_SNAPSHOT_SCHEMA.to_string(),
            strategy_id: "strategy_1".to_string(),
            strategy_version_id: Some("version_1".to_string()),
            source: DatasetSourceManifest {
                source_class: DatasetSourceClass::Test,
                plugin_instance_ref: "local-data".to_string(),
                plugin_ref: "tradeassembly.local-data".to_string(),
                operation_id: "marketdata.bars.read_v1".to_string(),
                plugin_manifest_fingerprint: "sha256:manifest".to_string(),
                capability_graph_revision_id: "graph_1".to_string(),
                capability_graph_fingerprint: "sha256:graph".to_string(),
                source_request_hash: "sha256:request".to_string(),
                upstream_refs: BTreeMap::new(),
            },
            asset_classes: vec![AssetClass::CryptoSpot],
            instruments: vec!["BTC/USD".to_string()],
            data_kind: HistoricalDataKind::Bars,
            granularity: "1m".to_string(),
            time_slice: DatasetTimeSlice {
                start: "2026-01-02T00:00:00Z".to_string(),
                end: "2026-01-02T00:02:00Z".to_string(),
            },
            calendar: "crypto_24x7".to_string(),
            timezone: "UTC".to_string(),
            normalization_policy: DatasetNormalizationPolicy {
                timestamp_unit: "rfc3339".to_string(),
                timezone: "UTC".to_string(),
                duplicate_policy: "reject".to_string(),
                price_adjustment: "raw".to_string(),
            },
            quality_policy: DatasetQualityPolicy {
                missing_intervals: QualityDisposition::Warn,
                stale_observations: QualityDisposition::Reject,
                outliers: QualityDisposition::Warn,
                invalid_markets: QualityDisposition::Reject,
                calendar_mismatch: QualityDisposition::Reject,
                corporate_action_gaps: QualityDisposition::Warn,
                missing_derivative_fields: QualityDisposition::Reject,
            },
            quality_findings: vec![],
            observations: vec![
                HistoricalObservation {
                    instrument_id: "BTC/USD".to_string(),
                    timestamp: "2026-01-02T00:00:00Z".to_string(),
                    data: HistoricalObservationData::Bar(BarObservation {
                        open: 100.0,
                        high: 100.5,
                        low: 99.5,
                        close: 100.0,
                        volume: 10.0,
                        vwap: None,
                        session: None,
                    }),
                },
                HistoricalObservation {
                    instrument_id: "BTC/USD".to_string(),
                    timestamp: "2026-01-02T00:01:00Z".to_string(),
                    data: HistoricalObservationData::Bar(BarObservation {
                        open: 100.0,
                        high: 101.5,
                        low: 99.5,
                        close: 101.0,
                        volume: 12.0,
                        vwap: None,
                        session: None,
                    }),
                },
            ],
        }
    }

    #[test]
    fn optional_provider_vwap_preserves_legacy_wire_shape_and_binds_snapshot_identity() {
        let legacy = serde_json::json!({
            "open": 100.0, "high": 100.5, "low": 99.5, "close": 100.0, "volume": 10.0
        });
        let bar: BarObservation = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(bar.vwap, None);
        assert_eq!(serde_json::to_value(bar).unwrap(), legacy);

        let original = DatasetSnapshot::build(content(), 1).unwrap();
        let mut with_vwap = content();
        let HistoricalObservationData::Bar(bar) = &mut with_vwap.observations[0].data else {
            panic!("expected bar");
        };
        bar.vwap = Some(100.25);
        let enriched = DatasetSnapshot::build(with_vwap, 1).unwrap();
        assert_ne!(original.content_hash, enriched.content_hash);
        let decoded: DatasetSnapshot =
            serde_json::from_value(serde_json::to_value(&enriched).unwrap()).unwrap();
        decoded.verify().unwrap();
        assert_eq!(decoded, enriched);
    }

    #[test]
    fn invalid_provider_vwap_is_rejected_not_replaced_by_close() {
        for value in [f64::NAN, f64::INFINITY, 0.0, -1.0, 99.0, 101.0] {
            let mut candidate = content();
            let HistoricalObservationData::Bar(bar) = &mut candidate.observations[0].data else {
                panic!("expected bar");
            };
            bar.vwap = Some(value);
            assert!(DatasetSnapshot::build(candidate, 1).is_err());
        }
    }

    #[test]
    fn calendar_evidence_is_hash_bound_and_invalid_intervals_are_rejected() {
        let original = DatasetSnapshot::build(content(), 1).unwrap();
        let mut enriched = content();
        let calendar = enriched.calendar.clone();
        let HistoricalObservationData::Bar(bar) = &mut enriched.observations[0].data else {
            panic!("bar");
        };
        bar.session = Some(BarSession {
            calendar,
            start: "2026-01-02T00:00:00Z".into(),
            end: "2026-01-03T00:00:00Z".into(),
            source_ref: "test-calendar:2026-01-02".into(),
        });
        let snapshot = DatasetSnapshot::build(enriched.clone(), 1).unwrap();
        assert_ne!(original.content_hash, snapshot.content_hash);
        snapshot.verify().unwrap();
        let HistoricalObservationData::Bar(bar) = &mut enriched.observations[0].data else {
            panic!("bar");
        };
        bar.session.as_mut().unwrap().end = "2026-01-01T00:00:00Z".into();
        assert_eq!(
            DatasetSnapshot::build(enriched, 1).unwrap_err(),
            "dataset_snapshot_session_invalid"
        );
    }

    #[test]
    fn snapshot_identity_is_content_addressed_and_order_independent() {
        let first = DatasetSnapshot::build(content(), 1).expect("snapshot");
        let second = DatasetSnapshot::build(content(), 2).expect("snapshot");
        assert_eq!(first.dataset_id, second.dataset_id);
        assert_eq!(first.content_hash, second.content_hash);
        first.verify().expect("integrity");
    }

    #[test]
    fn snapshot_detects_content_and_envelope_tampering() {
        let mut snapshot = DatasetSnapshot::build(content(), 1).expect("snapshot");
        let HistoricalObservationData::Bar(bar) = &mut snapshot.content.observations[0].data else {
            panic!("bar observation expected");
        };
        bar.close = 999.0;
        assert_eq!(
            snapshot.verify().expect_err("tamper rejected"),
            "dataset_snapshot_integrity_failed"
        );

        let mut snapshot = DatasetSnapshot::build(content(), 1).expect("snapshot");
        snapshot.row_count += 1;
        assert_eq!(
            snapshot.verify().expect_err("envelope rejected"),
            "dataset_snapshot_integrity_failed"
        );
    }

    #[test]
    fn snapshot_rejects_duplicate_or_unordered_rows() {
        let mut invalid = content();
        invalid.observations.swap(0, 1);
        assert_eq!(
            DatasetSnapshot::build(invalid, 1).expect_err("unordered rejected"),
            "dataset_snapshot_rows_unordered"
        );
    }

    #[test]
    fn snapshot_uses_global_time_bounds_for_instrument_grouped_rows() {
        let mut grouped = content();
        grouped.instruments.push("ETH/USD".to_string());
        grouped.observations.push(HistoricalObservation {
            instrument_id: "ETH/USD".to_string(),
            timestamp: "2026-01-02T00:00:30Z".to_string(),
            data: HistoricalObservationData::Bar(BarObservation {
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                volume: 5.0,
                vwap: None,
                session: None,
            }),
        });
        let snapshot = DatasetSnapshot::build(grouped, 1).expect("snapshot");
        assert_eq!(snapshot.first_timestamp, "2026-01-02T00:00:00Z");
        assert_eq!(snapshot.last_timestamp, "2026-01-02T00:01:00Z");
    }

    #[test]
    fn snapshot_enforces_bar_and_option_schemas() {
        let mut invalid_bar = content();
        let HistoricalObservationData::Bar(bar) = &mut invalid_bar.observations[0].data else {
            panic!("bar expected");
        };
        bar.high = 1.0;
        assert_eq!(
            DatasetSnapshot::build(invalid_bar, 1).expect_err("bad bar"),
            "dataset_snapshot_bar_invalid"
        );

        let mut option = content();
        option.asset_classes = vec![AssetClass::Option];
        option.instruments = vec!["SPY".to_string()];
        option.data_kind = HistoricalDataKind::OptionChain;
        option.observations = vec![HistoricalObservation {
            instrument_id: "SPY260102C00600000".to_string(),
            timestamp: "2026-01-02T00:00:00Z".to_string(),
            data: HistoricalObservationData::OptionContract(Box::new(OptionContractObservation {
                contract_symbol: "SPY260102C00600000".to_string(),
                underlying_instrument_id: "SPY".to_string(),
                expiration: "2026-01-02".to_string(),
                strike: 600.0,
                right: OptionRight::Call,
                bid: Some(2.0),
                ask: Some(2.1),
                last: Some(2.05),
                volume: Some(100.0),
                open_interest: Some(500.0),
                implied_volatility: Some(0.2),
                greeks: Some(OptionGreeks {
                    delta: Some(0.5),
                    gamma: Some(0.02),
                    theta: Some(-0.1),
                    vega: Some(0.2),
                    rho: None,
                }),
            })),
        }];
        DatasetSnapshot::build(option.clone(), 1).expect("valid option");

        let HistoricalObservationData::OptionContract(contract) = &mut option.observations[0].data
        else {
            panic!("option expected");
        };
        contract.ask = Some(1.0);
        assert_eq!(
            DatasetSnapshot::build(option, 1).expect_err("crossed option"),
            "dataset_snapshot_option_invalid"
        );
    }
}
