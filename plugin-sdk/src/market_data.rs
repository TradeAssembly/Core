//! Provider-neutral OHLCV values and explicit event/provenance validation.
//! This codec performs no acquisition, publication-time inference, provider
//! substitution, aggregation, price adjustment or indicator calculation.
use crate::{
    canonical::{canonical_sha256, MAX_EXACT_INTEGER},
    data_contract::*,
    data_evaluation::*,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const OHLCV_SERIES_SCHEMA: &str = "schema://tradeassembly/ohlcv-series@1";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OhlcvBar {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vwap: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<BarSession>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BarSession {
    pub calendar: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub source_ref: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBar {
    pub instrument_id: String,
    pub source_event_id: String,
    pub provenance: BatchProvenance,
    pub value: OhlcvBar,
}

pub fn ohlcv_series_schema() -> Value {
    json!({"type":"array","items":{"type":"object","properties":{
        "open":{"type":"number"},"high":{"type":"number"},"low":{"type":"number"},"close":{"type":"number"},"volume":{"type":"number","minimum":0},"vwap":{"type":"number"},"tradeCount":{"type":"integer","minimum":0,"maximum":MAX_EXACT_INTEGER},
        "session":{"type":"object","properties":{"calendar":{"type":"string","minLength":1},"startMs":{"type":"integer"},"endMs":{"type":"integer"},"sourceRef":{"type":"string","minLength":1}},"required":["calendar","startMs","endMs","sourceRef"],"additionalProperties":false}
    },"required":["open","high","low","close","volume"],"additionalProperties":false}})
}
pub fn ohlcv_identity() -> SchemaIdentity {
    SchemaIdentity {
        schema_ref: OHLCV_SERIES_SCHEMA.into(),
        sha256: canonical_sha256(&ohlcv_series_schema()).expect("fixed schema"),
    }
}
pub fn ohlcv_output_port(interval_ms: u64) -> Result<DataPort, String> {
    if interval_ms == 0 || interval_ms > MAX_EXACT_INTEGER {
        return Err("provider_bar_interval_invalid".into());
    }
    Ok(DataPort {
        schema: ohlcv_identity(),
        shape: DataShape::Series,
        time: TimeContract {
            interval_ms: Some(interval_ms),
            completed_only: true,
        },
    })
}
impl OhlcvBar {
    pub fn validate(&self) -> Result<(), String> {
        if [self.open, self.high, self.low, self.close, self.volume]
            .into_iter()
            .chain(self.vwap)
            .any(|v| !v.is_finite())
            || self.volume < 0.0
            || self.low > self.high
            || self.open < self.low
            || self.open > self.high
            || self.close < self.low
            || self.close > self.high
            || self.trade_count.is_some_and(|n| n > MAX_EXACT_INTEGER)
            || self.session.as_ref().is_some_and(|s| {
                s.calendar.trim().is_empty()
                    || s.source_ref.trim().is_empty()
                    || s.start_ms >= s.end_ms
            })
        {
            return Err("provider_bar_values_invalid".into());
        }
        canonical_sha256(self).map_err(|_| "provider_bar_numeric_range_invalid")?;
        Ok(())
    }
}
/// Builds one ordered bounded batch. The caller must supply factual availability
/// and source revision metadata; an event end is not an inferred publication time.
/// Empty batches are explicit no-data results, never fabricated observations.
pub fn bar_batch(
    request: &DataEvaluationRequest,
    interval_ms: u64,
    rows: Vec<ProviderBar>,
) -> Result<DataBatch, String> {
    let port = ohlcv_output_port(interval_ms)?;
    let instruments = request
        .instruments
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if instruments.is_empty()
        || instruments.len() != request.instruments.len()
        || rows.len() > 256
        || rows.len() as u64 > request.max_points
        || request.window.event_start_ms >= request.window.event_end_ms
        || request.window.event_end_ms > request.window.as_of_ms
    {
        return Err("provider_bar_request_invalid".into());
    }
    let mut previous = None;
    let mut identities = BTreeSet::new();
    let mut points = Vec::with_capacity(rows.len());
    for row in rows {
        row.value.validate()?;
        let p = &row.provenance;
        let order = (p.event_end_ms, row.instrument_id.clone());
        if !instruments.contains(row.instrument_id.as_str())
            || row.source_event_id.trim().is_empty()
            || p.source_revision.trim().is_empty()
            || p.event_start_ms < request.window.event_start_ms
            || p.event_end_ms > request.window.event_end_ms
            || p.event_end_ms
                .checked_sub(p.event_start_ms)
                .and_then(|n| u64::try_from(n).ok())
                != Some(interval_ms)
            || p.known_at_ms < p.event_end_ms
            || p.known_at_ms > request.window.as_of_ms
            || p.arrived_at_ms < p.known_at_ms
            || previous.as_ref().is_some_and(|before| before > &order)
            || !identities.insert(order.clone())
        {
            return Err("provider_bar_event_invalid".into());
        }
        previous = Some(order);
        let mut point = DataPoint {
            instrument_id: row.instrument_id,
            source_event_id: row.source_event_id,
            provenance: row.provenance,
            result: PointValue::Ready(
                serde_json::to_value(row.value).map_err(|_| "provider_bar_values_invalid")?,
            ),
        };
        point.provenance.content_sha256 =
            point_sha256(&point).map_err(|_| "provider_bar_provenance_invalid")?;
        points.push(point);
    }
    Ok(DataBatch {
        schema: port.schema,
        shape: port.shape,
        points,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> DataEvaluationRequest {
        DataEvaluationRequest {
            schema_version: EvaluationVersion::V1,
            plan_sha256: "plan".into(),
            node_id: "source".into(),
            semantics_version: "provider.explicit-acquisition.v1".into(),
            instruments: vec!["BTC/USD".into()],
            window: EvaluationWindow {
                event_start_ms: 10,
                event_end_ms: 12,
                as_of_ms: 20,
            },
            parameters: json!({}),
            inputs: Default::default(),
            prior_state: Default::default(),
            max_points: 2,
        }
    }
    fn row() -> ProviderBar {
        ProviderBar {
            instrument_id: "BTC/USD".into(),
            source_event_id: "provider-event".into(),
            provenance: BatchProvenance {
                content_sha256: String::new(),
                upstream_content_sha256: vec![],
                event_start_ms: 10,
                event_end_ms: 11,
                known_at_ms: 15,
                arrived_at_ms: 16,
                completed: true,
                source_revision: "provider-revision.1".into(),
            },
            value: OhlcvBar {
                open: 100.0,
                high: 102.0,
                low: 99.0,
                close: 101.0,
                volume: 5.0,
                vwap: Some(100.5),
                trade_count: Some(7),
                session: None,
            },
        }
    }
    #[test]
    fn factual_availability_and_optional_provider_fields_are_preserved() {
        let batch = bar_batch(&request(), 1, vec![row()]).unwrap();
        let point = &batch.points[0];
        assert_eq!(point.provenance.known_at_ms, 15);
        assert_eq!(point.provenance.arrived_at_ms, 16);
        let PointValue::Ready(value) = &point.result else {
            panic!("ready bar")
        };
        assert_eq!(value["vwap"], 100.5);
        assert_eq!(value["tradeCount"], 7);
        assert_eq!(
            point_sha256(point).unwrap(),
            point.provenance.content_sha256
        );
        let mut missing = row();
        missing.value.vwap = None;
        missing.value.trade_count = None;
        let batch = bar_batch(&request(), 1, vec![missing]).unwrap();
        let PointValue::Ready(value) = &batch.points[0].result else {
            panic!("ready bar")
        };
        assert!(value.get("vwap").is_none());
        assert!(value.get("tradeCount").is_none());
    }
    #[test]
    fn acquisition_after_cutoff_is_rejected_instead_of_relabelled_as_known_at_close() {
        let mut cutoff = request();
        cutoff.window.as_of_ms = 12;
        assert_eq!(
            bar_batch(&cutoff, 1, vec![row()]).unwrap_err(),
            "provider_bar_event_invalid"
        );
        let mut future = row();
        future.provenance.known_at_ms = 10;
        assert_eq!(
            bar_batch(&request(), 1, vec![future]).unwrap_err(),
            "provider_bar_event_invalid"
        );
        let mut revised = row();
        revised.provenance.source_revision = "provider-revision.2".into();
        assert_ne!(
            bar_batch(&request(), 1, vec![row()]).unwrap().points[0]
                .provenance
                .content_sha256,
            bar_batch(&request(), 1, vec![revised]).unwrap().points[0]
                .provenance
                .content_sha256
        );
    }
    #[test]
    fn malformed_unordered_duplicate_and_nonfinite_rows_fail() {
        assert!(bar_batch(&request(), 1, vec![row(), row()]).is_err());
        let mut bad = row();
        bad.value.high = 99.0;
        assert!(bar_batch(&request(), 1, vec![bad]).is_err());
        let mut bad = row();
        bad.value.volume = f64::NAN;
        assert!(bar_batch(&request(), 1, vec![bad]).is_err());
        let mut other = row();
        other.instrument_id = "unrequested".into();
        assert!(bar_batch(&request(), 1, vec![other]).is_err());
        let mut later = row();
        later.provenance.event_start_ms = 11;
        later.provenance.event_end_ms = 12;
        assert!(bar_batch(&request(), 1, vec![later, row()]).is_err());
    }
}
