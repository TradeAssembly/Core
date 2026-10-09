// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::historical_data::{
    BarObservation, DatasetSourceClass, HistoricalDataKind, HistoricalObservation,
    HistoricalObservationData, OptionContractObservation, OptionGreeks, OptionRight,
};
use crate::ports::{
    FailureMode, HistoricalDataPage, HistoricalDataPort, HistoricalDataRequest, IdempotencyKey,
    PluginOperationPort, PluginOperationRequest, PortDescriptor, PortKind, SideEffectContext,
    VersionedPort,
};
use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;

const LOCAL_FIXTURE_PAGE_ROWS: u64 = 1_000;

pub struct PluginHistoricalDataAdapter {
    operations: Arc<dyn PluginOperationPort>,
}

impl PluginHistoricalDataAdapter {
    pub fn new(operations: Arc<dyn PluginOperationPort>) -> Result<Self, String> {
        Ok(Self { operations })
    }
}

impl VersionedPort for PluginHistoricalDataAdapter {
    fn descriptors(&self) -> Vec<PortDescriptor> {
        let mut descriptor = PortDescriptor::new(PortKind::Plugins, "plugin.historical-data")
            .for_profiles(&["local", "self_hosted"])
            .with_capabilities(&[
                "dataset.historical.acquire",
                "dataset.historical.paginate",
                "dataset.historical.fixture",
            ]);
        descriptor.failure_mode = FailureMode::FailClosed;
        descriptor.secret_fields = vec!["credential_handle".to_string()];
        vec![descriptor]
    }
}

impl HistoricalDataPort for PluginHistoricalDataAdapter {
    fn acquire_page(
        &self,
        request: &HistoricalDataRequest,
        context: &SideEffectContext,
    ) -> Result<HistoricalDataPage, String> {
        validate_request(request)?;
        if request.plugin_ref == "tradeassembly.local-data" {
            local_fixture_page(request)
        } else {
            self.plugin_page(request, context)
        }
    }
}

impl PluginHistoricalDataAdapter {
    fn plugin_page(
        &self,
        request: &HistoricalDataRequest,
        context: &SideEffectContext,
    ) -> Result<HistoricalDataPage, String> {
        validate_credential_handle(request)?;
        let capability = match request.data_kind {
            HistoricalDataKind::Bars => "market_data.bars.read@1",
            HistoricalDataKind::OptionChain => "marketdata.options_chain",
        };
        let page_identity = serde_json::to_vec(&json!({
            "parentIdempotencyKey": context.idempotency_key.as_str(),
            "request": request,
        }))
        .map_err(|_| "dataset_request_invalid".to_string())?;
        let page_context = SideEffectContext::new(
            context.authority.clone(),
            IdempotencyKey::new(format!(
                "dataset-page-{}",
                &format!("{:x}", Sha256::digest(page_identity))[..32]
            ))
            .map_err(|_| "dataset_request_invalid".to_string())?,
        );
        let response = self.operations.invoke(
            &PluginOperationRequest {
                correlation_id: page_context.idempotency_key.as_str().to_string(),
                plugin_instance_ref: request.plugin_instance_ref.clone(),
                plugin_ref: request.plugin_ref.clone(),
                manifest_fingerprint: request.plugin_manifest_fingerprint.clone(),
                operation_id: request.operation_id.clone(),
                capability: capability.to_string(),
                capability_graph_revision_id: request.capability_graph_revision_id.clone(),
                capability_graph_fingerprint: request.capability_graph_fingerprint.clone(),
                strategy_id: String::new(),
                strategy_version_id: String::new(),
                strategy_spec_hash: String::new(),
                activation_id: String::new(),
                attempt_id: String::new(),
                evaluation_tick_id: String::new(),
                mode: "research".to_string(),
                purpose: "backtest_dataset_setup".to_string(),
                account_ref: None,
                timeout_ms: 30_000,
                fencing_token: None,
                input: json!({
                    "assetClass": request.asset_class,
                    "instruments": request.instruments,
                    "dataKind": request.data_kind,
                    "granularity": request.granularity,
                    "timeSlice": request.time_slice,
                    "calendar": request.calendar,
                    "timezone": request.timezone,
                    "normalizationPolicy": request.normalization_policy,
                    "qualityPolicy": request.quality_policy,
                    "pageToken": request.page_token,
                    "maxRows": request.max_rows,
                }),
                evidence_refs: vec![format!(
                    "tradeassembly://capability-graph/{}",
                    request.capability_graph_revision_id
                )],
            },
            &page_context,
        )?;
        serde_json::from_value(response.payload)
            .map_err(|_| "dataset_schema_unsupported".to_string())
    }
}

fn validate_credential_handle(request: &HistoricalDataRequest) -> Result<(), String> {
    if let Some(handle) = request.credential_handle.as_deref() {
        if handle != request.plugin_instance_ref {
            return Err("dataset_credential_missing".to_string());
        }
    }
    Ok(())
}

fn validate_request(request: &HistoricalDataRequest) -> Result<(), String> {
    if request.plugin_instance_ref.trim().is_empty()
        || request.plugin_ref.trim().is_empty()
        || request.operation_id.trim().is_empty()
        || request.instruments.is_empty()
        || request
            .instruments
            .iter()
            .any(|value| value.trim().is_empty())
        || request.time_slice.start >= request.time_slice.end
        || request.max_rows == 0
    {
        return Err("dataset_request_invalid".to_string());
    }
    let expected_operation = match request.data_kind {
        HistoricalDataKind::Bars => "marketdata.bars.read_v1",
        HistoricalDataKind::OptionChain => "marketdata.options_chain.read",
    };
    if request.operation_id != expected_operation {
        return Err("dataset_operation_unsupported".to_string());
    }
    parse_time(&request.time_slice.start)?;
    parse_time(&request.time_slice.end)?;
    interval(&request.granularity)?;
    Ok(())
}

fn local_fixture_page(request: &HistoricalDataRequest) -> Result<HistoricalDataPage, String> {
    let start = parse_time(&request.time_slice.start)?;
    let end = parse_time(&request.time_slice.end)?;
    let step = interval(&request.granularity)?;
    let offset = request
        .page_token
        .as_deref()
        .unwrap_or("0")
        .parse::<u64>()
        .map_err(|_| "dataset_request_invalid".to_string())?;
    let page_size = request.max_rows.min(LOCAL_FIXTURE_PAGE_ROWS);
    let (observations, total_rows) =
        fixture_observations_page(request, start, end, step, offset, page_size)?;
    if offset > total_rows {
        return Err("dataset_request_invalid".to_string());
    }
    let end_offset = offset
        .checked_add(observations.len() as u64)
        .ok_or_else(|| "dataset_response_too_large".to_string())?;
    let next_page_token = (end_offset < total_rows).then(|| end_offset.to_string());
    Ok(HistoricalDataPage {
        source_class: DatasetSourceClass::Test,
        observations,
        next_page_token,
        upstream_refs: BTreeMap::from([(
            "fixture_id".to_string(),
            "tradeassembly.local-data.v1".to_string(),
        )]),
    })
}

fn fixture_observations_page(
    request: &HistoricalDataRequest,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    step: ChronoDuration,
    offset: u64,
    limit: u64,
) -> Result<(Vec<HistoricalObservation>, u64), String> {
    let total_rows = match request.data_kind {
        HistoricalDataKind::Bars => {
            let step_ms = step.num_milliseconds();
            let span_ms = (end - start).num_milliseconds();
            if step_ms <= 0 || span_ms < 0 {
                return Err("dataset_time_slice_invalid".to_string());
            }
            let rows_per_instrument = u64::try_from(span_ms / step_ms)
                .map_err(|_| "dataset_response_too_large".to_string())?
                .checked_add(1)
                .ok_or_else(|| "dataset_response_too_large".to_string())?;
            rows_per_instrument
                .checked_mul(request.instruments.len() as u64)
                .ok_or_else(|| "dataset_response_too_large".to_string())?
        }
        HistoricalDataKind::OptionChain => {
            if request.asset_class != crate::historical_data::AssetClass::Option {
                return Err("dataset_instrument_unsupported".to_string());
            }
            request.instruments.len() as u64
        }
    };
    if offset > total_rows {
        return Err("dataset_request_invalid".to_string());
    }
    let end_offset = offset.saturating_add(limit).min(total_rows);
    let mut observations = Vec::with_capacity(
        usize::try_from(end_offset.saturating_sub(offset))
            .map_err(|_| "dataset_response_too_large".to_string())?,
    );
    for flat_index in offset..end_offset {
        match request.data_kind {
            HistoricalDataKind::Bars => {
                let step_ms = step.num_milliseconds();
                let rows_per_instrument = u64::try_from((end - start).num_milliseconds() / step_ms)
                    .map_err(|_| "dataset_response_too_large".to_string())?
                    + 1;
                let instrument_index = usize::try_from(flat_index / rows_per_instrument)
                    .map_err(|_| "dataset_response_too_large".to_string())?;
                let row = flat_index % rows_per_instrument;
                let row_i64 =
                    i64::try_from(row).map_err(|_| "dataset_response_too_large".to_string())?;
                let delta_ms = step_ms
                    .checked_mul(row_i64)
                    .ok_or_else(|| "dataset_response_too_large".to_string())?;
                let timestamp = start
                    .checked_add_signed(ChronoDuration::milliseconds(delta_ms))
                    .ok_or_else(|| "dataset_time_slice_invalid".to_string())?;
                let instrument = &request.instruments[instrument_index];
                let base = 100.0 + instrument_index as f64 * 10.0 + row as f64 * 0.25;
                observations.push(HistoricalObservation {
                    instrument_id: instrument.clone(),
                    timestamp: canonical_time(timestamp),
                    data: HistoricalObservationData::Bar(BarObservation {
                        open: base,
                        high: base + 1.0,
                        low: base - 1.0,
                        close: base + 0.5,
                        volume: 1_000.0 + row as f64,
                        vwap: None,
                        session: None,
                    }),
                });
            }
            HistoricalDataKind::OptionChain => {
                let index = usize::try_from(flat_index)
                    .map_err(|_| "dataset_response_too_large".to_string())?;
                let underlying = &request.instruments[index];
                let contract = format!("{}260117C00100000", underlying.replace('/', ""));
                observations.push(HistoricalObservation {
                    instrument_id: contract.clone(),
                    timestamp: canonical_time(start),
                    data: HistoricalObservationData::OptionContract(Box::new(
                        OptionContractObservation {
                            contract_symbol: contract,
                            underlying_instrument_id: underlying.clone(),
                            expiration: "2026-01-17".to_string(),
                            strike: 100.0 + index as f64 * 5.0,
                            right: OptionRight::Call,
                            bid: Some(2.0),
                            ask: Some(2.2),
                            last: Some(2.1),
                            volume: Some(100.0),
                            open_interest: Some(500.0),
                            implied_volatility: Some(0.25),
                            greeks: Some(OptionGreeks {
                                delta: Some(0.5),
                                gamma: Some(0.02),
                                theta: Some(-0.03),
                                vega: Some(0.1),
                                rho: Some(0.01),
                            }),
                        },
                    )),
                });
            }
        }
    }
    Ok((observations, total_rows))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| "dataset_time_slice_invalid".to_string())
}

fn canonical_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn interval(value: &str) -> Result<ChronoDuration, String> {
    let (number, unit) = value.split_at(value.len().saturating_sub(1));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::historical_data::{
        AssetClass, DatasetNormalizationPolicy, DatasetQualityPolicy, DatasetTimeSlice,
        QualityDisposition,
    };

    fn request(data_kind: HistoricalDataKind, asset_class: AssetClass) -> HistoricalDataRequest {
        HistoricalDataRequest {
            plugin_instance_ref: "local-data".to_string(),
            plugin_ref: "tradeassembly.local-data".to_string(),
            plugin_manifest_fingerprint: "sha256:local-data".to_string(),
            capability_graph_revision_id: "capgraph-local-data".to_string(),
            capability_graph_fingerprint: "sha256:capgraph-local-data".to_string(),
            operation_id: match data_kind {
                HistoricalDataKind::Bars => "marketdata.bars.read_v1",
                HistoricalDataKind::OptionChain => "marketdata.options_chain.read",
            }
            .to_string(),
            asset_class,
            instruments: vec!["SPY".to_string()],
            data_kind,
            granularity: "1m".to_string(),
            time_slice: DatasetTimeSlice {
                start: "2026-01-02T00:00:00Z".to_string(),
                end: "2026-01-02T00:02:00Z".to_string(),
            },
            calendar: "XNYS".to_string(),
            timezone: "America/New_York".to_string(),
            normalization_policy: DatasetNormalizationPolicy {
                timestamp_unit: "rfc3339".to_string(),
                timezone: "UTC".to_string(),
                duplicate_policy: "reject".to_string(),
                price_adjustment: "split_dividend_adjusted".to_string(),
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
            credential_handle: None,
            page_token: None,
            max_rows: 100,
        }
    }

    #[test]
    fn local_fixture_is_explicitly_test_data_and_typed() {
        let page = local_fixture_page(&request(HistoricalDataKind::Bars, AssetClass::Equity))
            .expect("fixture bars");
        assert_eq!(page.source_class, DatasetSourceClass::Test);
        assert_eq!(page.observations.len(), 3);
        assert!(page
            .observations
            .iter()
            .all(|row| matches!(row.data, HistoricalObservationData::Bar(_))));

        let options = local_fixture_page(&request(
            HistoricalDataKind::OptionChain,
            AssetClass::Option,
        ))
        .expect("fixture options");
        assert_eq!(options.source_class, DatasetSourceClass::Test);
        assert!(matches!(
            options.observations[0].data,
            HistoricalObservationData::OptionContract(_)
        ));
    }

    #[test]
    fn operation_mismatch_fails_closed() {
        let mut invalid = request(HistoricalDataKind::Bars, AssetClass::Equity);
        invalid.operation_id = "marketdata.options_chain.read".to_string();
        assert_eq!(
            validate_request(&invalid).expect_err("operation mismatch"),
            "dataset_operation_unsupported"
        );
    }

    #[test]
    fn external_data_credentials_are_optional_only_when_resolution_omits_a_handle() {
        let mut external = request(HistoricalDataKind::Bars, AssetClass::Equity);
        external.plugin_instance_ref = "marketdata-app".to_string();
        external.plugin_ref = "tradeassembly.marketdata-app".to_string();

        validate_credential_handle(&external).expect("credential-free operation");

        external.credential_handle = Some("another-instance".to_string());
        assert_eq!(
            validate_credential_handle(&external).expect_err("mismatched grant"),
            "dataset_credential_missing"
        );

        external.credential_handle = Some("marketdata-app".to_string());
        validate_credential_handle(&external).expect("matching credential grant");
    }

    #[test]
    fn local_fixture_paginates_and_supports_crypto_and_options_without_production_claims() {
        let mut crypto = request(HistoricalDataKind::Bars, AssetClass::CryptoSpot);
        crypto.instruments = vec!["BTC/USD".to_string()];
        crypto.calendar = "crypto_24x7".to_string();
        crypto.timezone = "UTC".to_string();
        crypto.max_rows = 2;
        let first = local_fixture_page(&crypto).expect("first page");
        assert_eq!(first.source_class, DatasetSourceClass::Test);
        assert_eq!(first.observations.len(), 2);
        assert_eq!(first.next_page_token.as_deref(), Some("2"));
        crypto.page_token = first.next_page_token;
        let second = local_fixture_page(&crypto).expect("second page");
        assert_eq!(second.observations.len(), 1);
        assert!(second.next_page_token.is_none());

        let options = local_fixture_page(&request(
            HistoricalDataKind::OptionChain,
            AssetClass::Option,
        ))
        .expect("options");
        assert_eq!(options.source_class, DatasetSourceClass::Test);
        assert!(matches!(
            options.observations[0].data,
            HistoricalObservationData::OptionContract(_)
        ));
    }

    #[test]
    fn local_fixture_bounds_generation_to_the_requested_page() {
        let mut large = request(HistoricalDataKind::Bars, AssetClass::CryptoSpot);
        large.instruments = vec!["BTC/USD".to_string()];
        large.calendar = "crypto_24x7".to_string();
        large.timezone = "UTC".to_string();
        large.time_slice.end = "2036-01-02T00:00:00Z".to_string();
        large.max_rows = 100;

        let page = local_fixture_page(&large).expect("bounded page");
        assert_eq!(page.observations.len(), 100);
        assert_eq!(page.next_page_token.as_deref(), Some("100"));
        assert_eq!(
            page.observations.last().expect("last row").timestamp,
            "2026-01-02T01:39:00.000Z"
        );
    }
}
